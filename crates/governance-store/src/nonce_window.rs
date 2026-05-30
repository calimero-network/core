//! Per-(group, signer) applied-nonce window.
//!
//! Governance ops carry a per-signer monotonic `nonce`. Applying them
//! used to be gated by a single high-water mark (`nonce <= last` → skip).
//! That dedup is correct only when ops apply in nonce order, which causal
//! DAG delivery guarantees *when a signer authors sequentially* — op N+1
//! has op N as a DAG ancestor, so it's buffered until N applies.
//!
//! It does NOT hold when the same signer authors two consecutive-nonce
//! ops **concurrently** (two sessions/devices reading the same DAG head):
//! the ops are DAG siblings (neither is the other's parent), so causal
//! ordering imposes no order between them. If nonce N+1 lands first the
//! high-water mark advances to N+1, and the later-arriving nonce N then
//! hits `nonce <= last` and is dropped permanently — the #2516 divergence.
//!
//! [`NonceWindow`] fixes this by tracking applied nonces tolerant of
//! out-of-order arrival: a contiguous `floor` (every nonce `1..=floor`
//! applied) plus a sparse set of applied nonces *above* the floor (the
//! concurrency gap, bounded by in-flight concurrency). A nonce is a
//! duplicate iff it's `<= floor` or already in the set; otherwise it's
//! applied once and the floor advances through any now-contiguous run.
//!
//! Persistence keeps `floor` under the existing `GroupLocalGovNonce` key
//! (so old single-`u64` rows are read as `floor` with an empty set — no
//! migration) and the above-floor set under a sibling key.

use std::collections::BTreeSet;

/// Applied-nonce window for one (group, signer). See module docs.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NonceWindow {
    /// Highest nonce `N` such that every nonce in `1..=N` is applied.
    floor: u64,
    /// Applied nonces strictly greater than `floor` that aren't yet
    /// contiguous with it. Always `> floor`; collapses into `floor` as
    /// the gaps fill.
    above: BTreeSet<u64>,
}

impl NonceWindow {
    /// Build a window from persisted parts, normalising so the invariant
    /// (`every element of `above` is `> floor`, and `floor + 1` is not in
    /// `above`) holds even if the stored parts were written by an older
    /// or buggier writer.
    #[must_use]
    pub fn new(floor: u64, above: impl IntoIterator<Item = u64>) -> Self {
        let mut window = Self {
            floor,
            above: above.into_iter().collect(),
        };
        window.normalise();
        window
    }

    /// Drop any below-floor entries and pull the floor up through a
    /// contiguous run starting at `floor + 1`.
    fn normalise(&mut self) {
        self.above.retain(|&n| n > self.floor);
        while self.above.remove(&(self.floor + 1)) {
            self.floor += 1;
        }
    }

    /// Highest contiguous applied nonce.
    #[must_use]
    pub fn floor(&self) -> u64 {
        self.floor
    }

    /// Applied nonces above the floor (for persistence).
    pub fn above(&self) -> impl Iterator<Item = u64> + '_ {
        self.above.iter().copied()
    }

    /// Has `nonce` already been applied? This is the dedup check that
    /// replaces the old `nonce <= last`.
    #[must_use]
    pub fn contains(&self, nonce: u64) -> bool {
        nonce <= self.floor || self.above.contains(&nonce)
    }

    /// Highest applied nonce across both the floor and the above-set. The
    /// op author assigns `max_applied() + 1` as the next nonce, so a
    /// session that has already seen out-of-order ops doesn't re-mint a
    /// nonce that's higher up in the window.
    #[must_use]
    pub fn max_applied(&self) -> u64 {
        self.above.iter().next_back().copied().unwrap_or(self.floor)
    }

    /// Record `nonce` as applied. Returns `true` if it was newly applied
    /// (the caller should apply the op), `false` if it was already present
    /// (a dedup — drop it). Advances the floor through any run that
    /// `nonce` made contiguous.
    pub fn record(&mut self, nonce: u64) -> bool {
        if self.contains(nonce) {
            return false;
        }
        let _inserted = self.above.insert(nonce);
        while self.above.remove(&(self.floor + 1)) {
            self.floor += 1;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequential_apply_advances_floor_no_gap() {
        let mut w = NonceWindow::default();
        for n in 1..=5 {
            assert!(w.record(n), "nonce {n} should be newly applied");
        }
        assert_eq!(w.floor(), 5);
        assert_eq!(w.above().count(), 0, "no gaps → nothing above the floor");
        assert_eq!(w.max_applied(), 5);
    }

    #[test]
    fn out_of_order_sibling_lower_nonce_still_applies() {
        // The #2516 scenario: floor=4, then nonce 6 lands before nonce 5.
        let mut w = NonceWindow::new(4, []);
        assert!(w.record(6), "6 is above the floor → newly applied");
        assert_eq!(w.floor(), 4, "floor cannot advance past the 5-gap");
        assert_eq!(w.above().collect::<Vec<_>>(), vec![6]);

        // The old high-water-mark guard would have dropped 5 here
        // (5 <= last=6). The window applies it.
        assert!(w.record(5), "5 fills the gap → newly applied, NOT dropped");
        assert_eq!(
            w.floor(),
            6,
            "filling the gap collapses 5 and 6 into the floor"
        );
        assert_eq!(w.above().count(), 0);
        assert_eq!(w.max_applied(), 6);
    }

    #[test]
    fn replays_are_deduped_in_any_order() {
        let mut w = NonceWindow::new(4, []);
        assert!(w.record(6));
        assert!(w.record(5));
        // Every nonce now applied; replays (the retry-encrypted-ops path
        // re-feeds the whole log) must dedup.
        assert!(!w.record(5), "replay of 5 is a dedup");
        assert!(!w.record(6), "replay of 6 is a dedup");
        assert!(!w.record(4), "replay below floor is a dedup");
        assert!(!w.record(1), "replay far below floor is a dedup");
        assert_eq!(w.floor(), 6);
    }

    #[test]
    fn wide_gap_holds_until_filled() {
        let mut w = NonceWindow::new(4, []);
        assert!(w.record(8));
        assert!(w.record(6));
        assert_eq!(w.floor(), 4, "floor stuck behind the 5 and 7 gaps");
        assert_eq!(w.above().collect::<Vec<_>>(), vec![6, 8]);
        assert_eq!(w.max_applied(), 8, "author would mint nonce 9 next");

        assert!(w.record(5)); // fills 5 → floor=6 (6 contiguous), 7 still missing
        assert_eq!(w.floor(), 6);
        assert_eq!(w.above().collect::<Vec<_>>(), vec![8]);

        assert!(w.record(7)); // fills 7 → floor advances through 7,8
        assert_eq!(w.floor(), 8);
        assert_eq!(w.above().count(), 0);
    }

    #[test]
    fn new_normalises_inconsistent_persisted_parts() {
        // A persisted set that includes below-floor and floor+1 entries
        // (shouldn't happen, but be defensive): normalise on load.
        let w = NonceWindow::new(4, [2, 4, 5, 6, 9]);
        assert_eq!(w.floor(), 6, "5 and 6 are contiguous past floor=4");
        assert_eq!(w.above().collect::<Vec<_>>(), vec![9]);
        assert!(w.contains(2));
        assert!(w.contains(6));
        assert!(w.contains(9));
        assert!(!w.contains(7));
        assert!(!w.contains(8));
    }

    #[test]
    fn old_single_high_water_mark_loads_as_floor() {
        // Backward-compat: an old node stored just a u64 high-water mark.
        // Loaded as floor with an empty above-set, the behaviour matches
        // the old guard exactly (dedup everything <= the mark).
        let w = NonceWindow::new(7, []);
        assert_eq!(w.floor(), 7);
        assert!(w.contains(7));
        assert!(w.contains(1));
        assert!(!w.contains(8));
        assert_eq!(w.max_applied(), 7);
    }

    #[test]
    fn max_applied_empty_is_zero() {
        let w = NonceWindow::default();
        assert_eq!(w.max_applied(), 0, "fresh signer → next nonce is 1");
        assert!(!w.contains(1));
    }
}
