//! In-memory awareness store: LWW-by-`(author, seq)` with TTL expiry.
//!
//! **Deliberately import-free of network, storage, and actix.** This is the
//! structural "never persisted" boundary — a type with no RocksDB access
//! cannot write to disk. Tasks 6 (inbound) and 7 (outbound) drive this.
//!
//! Time is always passed in as `now_ms`; no wall-clock calls exist here so
//! the type is fully deterministic and unit-testable in isolation.

use std::collections::{BTreeMap, HashMap};

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use tracing::debug;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Maximum number of distinct authors held for a single context.
///
/// Bounds the memory one context's presence can pin: at most
/// `MAX_AUTHORS_PER_CONTEXT * EPHEMERAL_MAX_BYTES` (512 × 16 KiB = 8 MiB) of
/// slice bytes, plus per-entry overhead.
///
/// **This is a backstop, not the primary defence.** The receive path
/// authenticates that the claimed author's key signed the envelope, but not
/// that the author is a registered context member — so any holder of the
/// *current group key* can mint throwaway keypairs and insert one entry per
/// key, faster than the TTL sweep reclaims them. Membership already gates
/// possession of that key; the cap is what stops a member (or anyone who has
/// obtained the key) from turning that into unbounded growth on every
/// receiving node.
///
/// 512 sits far above any plausible real context — presence is a
/// cursors/typing/online channel for humans in one shared document — while
/// keeping the worst case a bounded few MiB.
pub const MAX_AUTHORS_PER_CONTEXT: usize = 512;

/// A single diff produced by a mutating operation on the store.
#[derive(Debug, PartialEq)]
pub enum Diff {
    Upsert { author: PublicKey, slice: Vec<u8> },
    Remove { author: PublicKey },
}

// ---------------------------------------------------------------------------
// Internal types
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Entry {
    slice: Vec<u8>,
    seq: u64,
    last_seen_ms: u64,
}

// ---------------------------------------------------------------------------
// AwarenessStore
// ---------------------------------------------------------------------------

/// Per-context map of `author -> {slice, seq, last_seen_ms}`.
#[derive(Debug)]
pub struct AwarenessStore {
    // Outer: context → per-author entries.
    // Inner: BTreeMap so PublicKey (which is Ord but not std::hash::Hash)
    // can be used as a key, and iteration order is deterministic (sorted by
    // author bytes), which `snapshot` exploits for free.
    inner: HashMap<ContextId, BTreeMap<PublicKey, Entry>>,
}

impl AwarenessStore {
    /// Create an empty store.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Apply an incoming awareness slice for `author` in `ctx`.
    ///
    /// LWW rule: if an entry with `seq >= incoming_seq` already exists the
    /// call is a no-op and returns `None`.
    ///
    /// On accept the entry is updated. Returns `Some(Diff::Upsert)` **only**
    /// if the live slice bytes changed; a same-bytes re-apply with a higher
    /// `seq` updates liveness but returns `None` (no visible diff).
    ///
    /// A *new* author is refused once the context already holds
    /// [`MAX_AUTHORS_PER_CONTEXT`] of them. An author already present is always
    /// allowed to update, full map or not — otherwise a flood would freeze
    /// every real participant's presence at whatever it was when the map
    /// filled, which is worse than dropping the flood.
    ///
    /// The local-echo path (`set_local_ephemeral`) also lands here, so a node
    /// setting its own presence for the first time into a context whose map is
    /// full sees no echo. The publish still goes out — only the local echo is
    /// skipped — and the TTL sweep frees the map within
    /// [`crate::handlers::ephemeral::PRESENCE_TTL_MS`], so the next heartbeat
    /// re-applies it.
    pub fn apply(
        &mut self,
        ctx: ContextId,
        author: PublicKey,
        seq: u64,
        slice: Vec<u8>,
        now_ms: u64,
    ) -> Option<Diff> {
        let per_ctx = self.inner.entry(ctx).or_default();

        if let Some(entry) = per_ctx.get_mut(&author) {
            // Stale or equal seq → no-op.
            if seq <= entry.seq {
                return None;
            }
            let slice_changed = entry.slice != slice;
            entry.seq = seq;
            entry.last_seen_ms = now_ms;
            if slice_changed {
                entry.slice = slice.clone();
                return Some(Diff::Upsert { author, slice });
            }
            // Same bytes, higher seq → liveness updated, no diff.
            return None;
        }

        // New entry — the only case the author cap applies to.
        if per_ctx.len() >= MAX_AUTHORS_PER_CONTEXT {
            debug!(
                %ctx,
                %author,
                authors = per_ctx.len(),
                max = MAX_AUTHORS_PER_CONTEXT,
                "ephemeral: author cap reached for context — dropping presence from a new author"
            );
            return None;
        }
        per_ctx.insert(
            author,
            Entry {
                slice: slice.clone(),
                seq,
                last_seen_ms: now_ms,
            },
        );
        Some(Diff::Upsert { author, slice })
    }

    /// Refresh liveness for `author` in `ctx` without changing the slice.
    ///
    /// Called by the node's heartbeat tick for each of its OWN locally-set
    /// entries, immediately before the sweep. Remote entries are re-stamped by
    /// `apply` when their author's heartbeat arrives, but a node never
    /// receives its own gossip back, so this is the only thing that keeps a
    /// local author from being evicted by its own TTL sweep.
    ///
    /// A missing entry is silently ignored (the author may already have been
    /// swept).
    pub fn touch(&mut self, ctx: ContextId, author: PublicKey, now_ms: u64) {
        if let Some(per_ctx) = self.inner.get_mut(&ctx) {
            if let Some(entry) = per_ctx.get_mut(&author) {
                entry.last_seen_ms = now_ms;
            }
        }
    }

    /// Drop entries for `ctx` whose `last_seen_ms` is older than `ttl_ms`
    /// relative to `now_ms`. Returns one `Diff::Remove` per dropped entry.
    pub fn sweep(&mut self, ctx: ContextId, ttl_ms: u64, now_ms: u64) -> Vec<Diff> {
        let Some(per_ctx) = self.inner.get_mut(&ctx) else {
            return vec![];
        };

        let mut removals = Vec::new();
        per_ctx.retain(|author, entry| {
            if now_ms.saturating_sub(entry.last_seen_ms) >= ttl_ms {
                removals.push(Diff::Remove { author: *author });
                false
            } else {
                true
            }
        });
        removals
    }

    /// Snapshot of live slices for `ctx`, sorted by author bytes (stable order),
    /// each carrying how long it has been since that author was last heard from.
    ///
    /// Returns `(author, slice, age_ms)` where `age_ms = now_ms - last_seen_ms`.
    ///
    /// Age is reported **relative**, never as an absolute timestamp:
    /// `last_seen_ms` is stamped from *this* node's wall clock, so shipping it
    /// absolute would force a reader on another machine to subtract against its
    /// own clock and any skew between the two would corrupt the result.
    /// Computing the difference here keeps it skew-free.
    ///
    /// `age_ms` is bounded above by `PRESENCE_TTL_MS` for any entry the sweep
    /// has not yet removed, and in practice sits under `PRESENCE_HEARTBEAT_MS`
    /// for a live author.
    ///
    /// Returns an empty `Vec` when the context has no entries.
    pub fn snapshot(&self, ctx: ContextId, now_ms: u64) -> Vec<(PublicKey, Vec<u8>, u64)> {
        let Some(per_ctx) = self.inner.get(&ctx) else {
            return vec![];
        };
        // BTreeMap already iterates in sorted (author-bytes) order.
        per_ctx
            .iter()
            .map(|(author, entry)| {
                (
                    *author,
                    entry.slice.clone(),
                    now_ms.saturating_sub(entry.last_seen_ms),
                )
            })
            .collect()
    }

    /// All contexts currently holding at least one entry.
    ///
    /// Used to drive the TTL sweep: it must cover every context the store
    /// knows about, not just the ones a given node has locally published to
    /// (a receive-only node — a read-only viewer, a TEE node, a peer who has
    /// not yet moved their own cursor — never appears in the caller's local
    /// publish map, but its remote entries still need to expire on schedule).
    pub fn contexts(&self) -> impl Iterator<Item = ContextId> + '_ {
        self.inner.keys().copied()
    }

    /// Explicitly remove `author` from `ctx` (e.g. on disconnect).
    ///
    /// Returns `Some(Diff::Remove)` if the author was present, `None` otherwise.
    pub fn remove_author(&mut self, ctx: ContextId, author: PublicKey) -> Option<Diff> {
        let per_ctx = self.inner.get_mut(&ctx)?;
        per_ctx.remove(&author).map(|_| Diff::Remove { author })
    }
}

impl Default for AwarenessStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ContextId {
        ContextId::from([1u8; 32])
    }
    fn pk(n: u8) -> PublicKey {
        PublicKey::from([n; 32])
    }

    #[test]
    fn apply_then_snapshot() {
        let mut s = AwarenessStore::new();
        assert_eq!(
            s.apply(ctx(), pk(1), 1, vec![1], 1000),
            Some(Diff::Upsert {
                author: pk(1),
                slice: vec![1]
            })
        );
        // now_ms 1000 == the apply timestamp, so age is 0.
        assert_eq!(s.snapshot(ctx(), 1000), vec![(pk(1), vec![1], 0)]);
    }

    #[test]
    fn lww_ignores_stale_or_equal_seq() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 5, vec![5], 1000);
        assert!(s.apply(ctx(), pk(1), 4, vec![4], 1001).is_none()); // stale
        assert!(s.apply(ctx(), pk(1), 5, vec![9], 1002).is_none()); // equal seq
                                                                    // Stale/equal-seq applies do not touch last_seen_ms, which stayed at
                                                                    // 1000, so at now_ms 1500 the entry reads 500ms old.
        assert_eq!(s.snapshot(ctx(), 1500), vec![(pk(1), vec![5], 500)]);
    }

    #[test]
    fn sweep_expires_and_diffs() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 1, vec![1], 1000);
        assert!(s.sweep(ctx(), 7000, 5000).is_empty()); // still fresh
        assert_eq!(
            s.sweep(ctx(), 7000, 9000),
            vec![Diff::Remove { author: pk(1) }]
        ); // expired
        assert!(s.snapshot(ctx(), 9000).is_empty());
    }

    #[test]
    fn touch_extends_liveness() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 1, vec![1], 1000);
        s.touch(ctx(), pk(1), 6000);
        assert!(s.sweep(ctx(), 7000, 9000).is_empty()); // touched at 6000 → not expired at 9000
    }

    /// Fill a context to exactly `MAX_AUTHORS_PER_CONTEXT` distinct authors.
    /// Author keys are derived from the index so they are all distinct.
    fn fill_to_cap(s: &mut AwarenessStore, now_ms: u64) {
        for i in 0..MAX_AUTHORS_PER_CONTEXT {
            let mut bytes = [0u8; 32];
            bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
            // 0xFF marks these as flood authors, keeping them clear of the
            // single-byte `pk(n)` helper the other tests use.
            bytes[31] = 0xFF;
            assert!(
                s.apply(ctx(), PublicKey::from(bytes), 1, vec![1], now_ms)
                    .is_some(),
                "author {i} must be admitted while under the cap"
            );
        }
    }

    /// A holder of the current group key can mint unlimited throwaway keypairs
    /// — the receive path authenticates the signature, not membership — so
    /// inserts beyond the cap must be refused rather than grown into.
    #[test]
    fn new_authors_beyond_the_cap_are_refused() {
        let mut s = AwarenessStore::new();
        fill_to_cap(&mut s, 1000);
        assert_eq!(s.snapshot(ctx(), 1000).len(), MAX_AUTHORS_PER_CONTEXT);

        let intruder = PublicKey::from([0xAB; 32]);
        assert!(
            s.apply(ctx(), intruder, 1, vec![9], 1001).is_none(),
            "a new author must be refused once the context is at the cap"
        );
        assert_eq!(
            s.snapshot(ctx(), 1001).len(),
            MAX_AUTHORS_PER_CONTEXT,
            "the refusal must not have grown the map"
        );
    }

    /// The cap gates INSERTS only. An author already in the map must keep
    /// updating even when the map is full, or a flood would freeze every real
    /// participant's presence.
    #[test]
    fn an_existing_author_still_updates_when_the_map_is_full() {
        let mut s = AwarenessStore::new();
        let incumbent = pk(1);
        assert!(s.apply(ctx(), incumbent, 1, vec![1], 1000).is_some());
        // Fill the REMAINING slots, so the incumbent is one of the capped set.
        for i in 0..MAX_AUTHORS_PER_CONTEXT {
            let mut bytes = [0u8; 32];
            bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes[31] = 0xFF;
            let _ignored = s.apply(ctx(), PublicKey::from(bytes), 1, vec![1], 1000);
        }
        assert_eq!(s.snapshot(ctx(), 1000).len(), MAX_AUTHORS_PER_CONTEXT);

        assert_eq!(
            s.apply(ctx(), incumbent, 2, vec![7], 2000),
            Some(Diff::Upsert {
                author: incumbent,
                slice: vec![7]
            }),
            "an author already present must update at the cap"
        );
    }

    #[test]
    fn remove_author_diffs_once() {
        let mut s = AwarenessStore::new();
        s.apply(ctx(), pk(1), 1, vec![1], 1000);
        assert_eq!(
            s.remove_author(ctx(), pk(1)),
            Some(Diff::Remove { author: pk(1) })
        );
        assert!(s.remove_author(ctx(), pk(1)).is_none());
    }
}
