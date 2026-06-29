//! Admission-control throttle for the gossip `TeeAttestationAnnounce` path.
//!
//! Inbound `TeeAttestationAnnounce` broadcasts drive
//! [`tee_attestation_admission::handle_tee_attestation_announce`], which runs
//! the heavy [`calimero_tee_attestation::verify_attestation`] path — an
//! outbound Intel-PCS collateral fetch plus DCAP crypto-verify — *before* any
//! policy lookup. Without a guard, a malicious mesh peer on a TEE namespace
//! topic can replay a structurally-valid quote (varying the announce nonce to
//! beat gossipsub's message-id dedup) and amplify each cheap gossip frame into
//! a CPU verify + an outbound PCS request (TEE-01 / audit #48).
//!
//! This throttle is consulted *synchronously* on the `NodeManager` actor
//! thread before the verify task is spawned. It composes three independent
//! gates, each of which can reject an announce on its own:
//!
//! 1. **Per-group quote dedup** — a recently-seen `(group, quote_hash)` is
//!    dropped, so replaying one captured quote (identical `quote_bytes` ⇒
//!    identical hash) under many nonces costs at most one verify per TTL
//!    window. This complements the durable governance-store check
//!    (`is_quote_hash_used`, which only knows *admitted* quotes) by also
//!    covering not-yet-admitted replays.
//! 2. **Per-peer rate limit** — a lazily-refilled token bucket per source
//!    peer bounds how fast any single peer can drive verifies.
//! 3. **Global inflight cap** — a bounded semaphore caps the number of
//!    concurrent verifies across all peers/groups; the returned permit is
//!    held for the lifetime of the spawned verify task.
//!
//! The struct is touched only on the actor thread, so the bookkeeping maps
//! need no locking; only the inflight [`Semaphore`] is shared (it is moved,
//! via an owned permit, into the spawned task).
//!
//! Per-peer state is created only when a peer's announce actually *proceeds*
//! (Gate 3 grant), never on a rejection. A consequence is that a peer whose
//! announces are *always* refused at the global inflight cap accumulates no
//! per-peer rate-limit debt — but that is harmless by construction: such a peer
//! triggers zero verifies (the global cap is doing the limiting), and the
//! moment one of its announces proceeds it is tracked and metered normally.
//! Tracking never-proceeding peers would instead let a unique-peer flood grow
//! the map for no protective gain.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use libp2p::PeerId;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Maximum number of attestation verifies allowed to run concurrently across
/// all peers and groups. The verify path makes a blocking-ish outbound PCS
/// fetch, so this caps both CPU and outbound amplification fan-out.
pub const DEFAULT_MAX_INFLIGHT_VERIFIES: usize = 4;

/// Per-peer token-bucket burst: a single peer may trigger this many verifies
/// back-to-back before being throttled to the refill rate.
pub const DEFAULT_PER_PEER_BURST: f64 = 5.0;

/// Per-peer token-bucket refill interval — one token is restored per this
/// duration, up to [`DEFAULT_PER_PEER_BURST`]. At the default (1 token / 2s)
/// a saturating peer is held to ~30 verifies/min.
pub const DEFAULT_PER_PEER_REFILL: Duration = Duration::from_secs(2);

/// How long a `(group, quote_hash)` is remembered for dedup. A replay of the
/// same quote within this window is dropped without a verify.
pub const DEFAULT_DEDUP_TTL: Duration = Duration::from_secs(300);

/// Hard cap on the number of distinct peers and dedup entries retained, so a
/// flood of unique peers / quotes can't grow the maps without bound. When the
/// cap is hit, the oldest entries are pruned first.
const MAX_TRACKED_PEERS: usize = 4096;
const MAX_TRACKED_QUOTES: usize = 8192;

/// Minimum spacing between time-based [`TeeAdmissionThrottle::prune`] sweeps.
/// `prune`'s `retain` pass is O(tracked entries); running it on every announce
/// would make the per-call cost grow with the map size under an adversarial
/// flood — the exact case this throttle defends against. Instead we sweep at
/// most once per this interval (a size-cap guard still forces an immediate
/// sweep if either map exceeds its hard cap, so memory stays bounded).
const PRUNE_INTERVAL: Duration = Duration::from_secs(1);

/// Outcome of consulting the throttle for one announce.
#[derive(Debug)]
pub enum Decision {
    /// Proceed with the verify; hold `permit` until the verify completes so
    /// the global inflight cap stays accurate.
    Proceed(OwnedSemaphorePermit),
    /// The same `(group, quote_hash)` was seen recently — dropped.
    Duplicate,
    /// The source peer exceeded its per-peer rate limit — dropped.
    RateLimited,
    /// The global inflight-verify cap is saturated — dropped.
    AtCapacity,
}

struct PeerBucket {
    tokens: f64,
    /// Time of the last *granted* token. Drives the refill clock and is
    /// committed only on `Decision::Proceed` (see `check`), so a rejection
    /// never advances it.
    last: Instant,
    /// Time this peer was last *seen* (any `check` outcome, including
    /// rejections). Used as the idle/LRU key for pruning so an active-but-
    /// throttled peer is retained — evicting it would hand it a fresh full
    /// bucket and reset its rate limit.
    last_seen: Instant,
}

/// Admission-control throttle. See the module docs for the gate ordering and
/// rationale. Construct one per node and consult it on the actor thread.
pub struct TeeAdmissionThrottle {
    inflight: Arc<Semaphore>,
    peers: HashMap<PeerId, PeerBucket>,
    recent_quotes: HashMap<([u8; 32], [u8; 32]), Instant>,
    per_peer_burst: f64,
    per_peer_refill: Duration,
    dedup_ttl: Duration,
    /// `now` of the last time-based prune sweep, used to amortise `prune` to at
    /// most once per [`PRUNE_INTERVAL`]. `None` until the first `check`.
    last_prune: Option<Instant>,
}

impl std::fmt::Debug for TeeAdmissionThrottle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeeAdmissionThrottle")
            .field("available_permits", &self.inflight.available_permits())
            .field("tracked_peers", &self.peers.len())
            .field("tracked_quotes", &self.recent_quotes.len())
            .finish()
    }
}

impl Default for TeeAdmissionThrottle {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_INFLIGHT_VERIFIES,
            DEFAULT_PER_PEER_BURST,
            DEFAULT_PER_PEER_REFILL,
            DEFAULT_DEDUP_TTL,
        )
    }
}

impl TeeAdmissionThrottle {
    /// Construct a throttle with explicit gate parameters.
    ///
    /// # Panics
    ///
    /// Panics if `max_inflight == 0`, `per_peer_burst < 1.0`, or
    /// `per_peer_refill == 0`: with no inflight permits no announce could ever
    /// proceed, a sub-unit burst can never satisfy the `tokens >= 1.0` gate, and
    /// a zero refill interval would make the refill rate non-finite (poisoning
    /// the lazy-refill arithmetic with `NaN`) — so each renders the throttle
    /// useless. These are construction-time programmer errors — the only in-tree
    /// callers are [`Default`] and tests, both of which pass valid constants —
    /// so they are asserted rather than surfaced as a runtime `Result` the
    /// caller would have to thread through node startup.
    pub fn new(
        max_inflight: usize,
        per_peer_burst: f64,
        per_peer_refill: Duration,
        dedup_ttl: Duration,
    ) -> Self {
        assert!(max_inflight > 0, "max_inflight must be positive");
        assert!(per_peer_burst >= 1.0, "per_peer_burst must be >= 1");
        assert!(
            per_peer_refill > Duration::ZERO,
            "per_peer_refill must be positive"
        );
        Self {
            inflight: Arc::new(Semaphore::new(max_inflight)),
            peers: HashMap::new(),
            recent_quotes: HashMap::new(),
            per_peer_burst,
            per_peer_refill,
            dedup_ttl,
            last_prune: None,
        }
    }

    /// Tokens restored per second. `per_peer_refill` is asserted `> 0` in
    /// [`Self::new`], so this is always finite (no `1.0 / 0.0` / `INFINITY`,
    /// which would poison the `0 * rate` refill term with `NaN`).
    fn refill_per_sec(&self) -> f64 {
        1.0 / self.per_peer_refill.as_secs_f64()
    }

    /// Consult all three gates for an announce observed at `now`.
    ///
    /// On `Decision::Proceed` the `(group, quote_hash)` is recorded for dedup
    /// and one per-peer token + one inflight permit are consumed. On any
    /// rejection no token is consumed and nothing is recorded, so a legitimate
    /// retry after the rate-limit/capacity pressure clears is not penalised.
    pub fn check(
        &mut self,
        now: Instant,
        source: PeerId,
        group_id: [u8; 32],
        quote_hash: [u8; 32],
    ) -> Decision {
        // Prune is O(tracked entries); amortise it to at most once per
        // `PRUNE_INTERVAL` so an adversarial flood doesn't pay an O(N) sweep on
        // every announce before the cheap gates below can reject it. A size-cap
        // guard still forces an immediate sweep whenever either map is over its
        // hard cap, so memory stays bounded regardless of call cadence.
        let over_cap =
            self.recent_quotes.len() > MAX_TRACKED_QUOTES || self.peers.len() > MAX_TRACKED_PEERS;
        let due = self
            .last_prune
            .is_none_or(|last| now.saturating_duration_since(last) >= PRUNE_INTERVAL);
        if over_cap || due {
            self.prune(now);
            self.last_prune = Some(now);
        }

        // Gate 1: per-group quote dedup. Cheapest, and the most effective
        // guard against single-quote replay floods.
        let key = (group_id, quote_hash);
        if let Some(seen) = self.recent_quotes.get(&key) {
            if now.saturating_duration_since(*seen) < self.dedup_ttl {
                return Decision::Duplicate;
            }
        }

        // Gate 2: per-peer rate limit. Compute the lazily-refilled token count
        // but do NOT write it back yet — the bucket is mutated transactionally,
        // only on `Proceed` (see the commit block below). A rejected announce
        // (here or at Gate 3) therefore leaves `tokens` and `last` untouched, so
        // it neither burns a token nor advances the peer's refill clock.
        let burst = self.per_peer_burst;
        let refill_per_sec = self.refill_per_sec();
        // Read the current bucket state *without* inserting. A brand-new peer
        // is treated as a full bucket, so first contact is never rate-limited;
        // crucially, a peer is only inserted on the commit (Proceed) path below,
        // so a flood of unique source peers that never proceed can't grow
        // `peers`. For an already-tracked peer, refresh `last_seen` on every
        // call (any outcome) so an active-but-throttled peer isn't LRU-evicted.
        let (cur_tokens, cur_last) = match self.peers.get_mut(&source) {
            Some(bucket) => {
                bucket.last_seen = now;
                (bucket.tokens, bucket.last)
            }
            None => (burst, now),
        };
        let elapsed = now.saturating_duration_since(cur_last).as_secs_f64();
        let refilled = (cur_tokens + elapsed * refill_per_sec).min(burst);
        if refilled < 1.0 {
            return Decision::RateLimited;
        }

        // Gate 3: global inflight cap. Acquire last so a rejection here does
        // not burn a per-peer token. No bucket has been inserted/mutated for a
        // refused-here peer, so an `AtCapacity` return advances nothing.
        let permit = match Arc::clone(&self.inflight).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Decision::AtCapacity,
        };

        // All gates passed: commit the side effects atomically. This is the
        // only path that inserts/advances the bucket, so `tokens`/`last` reflect
        // the previous *grant* plus this one, never an intervening rejection.
        // The `or_insert_with` value is a placeholder used only for a genuinely
        // new peer; the three assignments below always run and set the real
        // post-grant state for both new and existing peers.
        let bucket = self.peers.entry(source).or_insert_with(|| PeerBucket {
            tokens: burst,
            last: now,
            last_seen: now,
        });
        bucket.tokens = refilled - 1.0;
        bucket.last = now;
        bucket.last_seen = now;
        let _ = self.recent_quotes.insert(key, now);
        Decision::Proceed(permit)
    }

    /// Forget a recorded `(group, quote_hash)` dedup entry so the next announce
    /// for that quote is re-verified instead of being suppressed for the full
    /// dedup TTL.
    ///
    /// Called after a verify that *errored* (a transient infrastructure failure
    /// — e.g. the Intel-PCS collateral fetch failed), so a legitimate re-announce
    /// of the identical quote (the fleet-join re-announce loop reuses the same
    /// `quote_bytes`) can recover within its admission window rather than being
    /// stuck until the TTL elapses. A quote that *failed verification* (a
    /// definite invalid result, not an error) is deliberately left recorded, so
    /// a replay of it stays suppressed. DoS remains bounded by the per-peer rate
    /// limit and the global inflight cap either way.
    pub fn forget_quote(&mut self, group_id: [u8; 32], quote_hash: [u8; 32]) {
        let _ = self.recent_quotes.remove(&(group_id, quote_hash));
    }

    /// Drop expired dedup entries and full/idle peer buckets, and hard-cap map
    /// sizes so adversarial churn can't grow memory without bound.
    fn prune(&mut self, now: Instant) {
        let dedup_ttl = self.dedup_ttl;
        self.recent_quotes
            .retain(|_, seen| now.saturating_duration_since(*seen) < dedup_ttl);
        // Hard cap. Reaching it requires >MAX_TRACKED_QUOTES *distinct* quotes
        // admitted within the TTL — and entries are only inserted on the commit
        // (Proceed) path, gated by the per-peer rate limit and the global
        // inflight cap, so a rejected flood can't grow this map at all. If the
        // cap is somehow hit, evicting the oldest entries by *insertion time*
        // (which, since the dedup TTL is uniform, are exactly those nearest to
        // expiry) degrades dedup to best-effort for those quotes (a replay could
        // trigger one more verify) — an intentional trade-off: the memory bound
        // is the hard guarantee, and the rate-limit + inflight gates (plus the
        // durable `is_quote_hash_used` check for admitted quotes) remain the
        // real DoS backstop. Do not "fix" this by removing the cap.
        if self.recent_quotes.len() > MAX_TRACKED_QUOTES {
            Self::evict_oldest(&mut self.recent_quotes, MAX_TRACKED_QUOTES);
        }

        // Drop a peer once it hasn't been *seen* for at least the time its
        // bucket needs to fully refill from empty (`burst * per_peer_refill`).
        // By then it is indistinguishable from a fresh peer, so its state is
        // worth nothing — and this is exactly the window after which a fresh
        // full bucket grants no more than the configured rate, so eviction can't
        // become a rate-limit-reset bypass (a peer flooding faster is *seen*
        // more recently and so retained). Keying on `last_seen` (updated every
        // call, any outcome) rather than `last` (grant time) is what keeps an
        // active-but-throttled peer; and because `last_seen >= last` always, a
        // peer idle this long has provably refilled, so no separate fullness
        // check is needed.
        let full_refill = self
            .per_peer_refill
            .saturating_mul(self.per_peer_burst.ceil() as u32);
        self.peers
            .retain(|_, b| now.saturating_duration_since(b.last_seen) < full_refill);
        if self.peers.len() > MAX_TRACKED_PEERS {
            Self::evict_oldest_peers(&mut self.peers, MAX_TRACKED_PEERS);
        }
    }

    fn evict_oldest<K: Clone + std::hash::Hash + Eq>(map: &mut HashMap<K, Instant>, keep: usize) {
        let mut entries: Vec<(K, Instant)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        Self::evict_oldest_entries(map, &mut entries, keep);
    }

    fn evict_oldest_peers(map: &mut HashMap<PeerId, PeerBucket>, keep: usize) {
        // Key on `last_seen` (last activity), not `last` (last grant), so the
        // peers dropped under cap pressure are the genuinely-quiet ones rather
        // than active-but-throttled peers (whose `last` is stale by design).
        let mut entries: Vec<(PeerId, Instant)> =
            map.iter().map(|(k, v)| (*k, v.last_seen)).collect();
        Self::evict_oldest_entries(map, &mut entries, keep);
    }

    /// Remove the oldest entries from `map` until at most `keep` remain. `entries`
    /// MUST be a full `(key, timestamp)` snapshot of `map` — both callers collect
    /// every entry immediately before calling — so `remove` (derived from
    /// `map.len()`) indexes the snapshot correctly. Uses `select_nth_unstable`
    /// (O(n) average) rather than a full O(n log n) sort, since the cap is only
    /// ever crossed under adversarial churn and the evicted entries are
    /// discarded, so their relative order is irrelevant.
    fn evict_oldest_entries<K: std::hash::Hash + Eq, V>(
        map: &mut HashMap<K, V>,
        entries: &mut [(K, Instant)],
        keep: usize,
    ) {
        debug_assert_eq!(
            entries.len(),
            map.len(),
            "evict_oldest_entries requires a full snapshot of `map`"
        );
        let remove = map.len().saturating_sub(keep);
        if remove == 0 {
            return;
        }
        let _ = entries.select_nth_unstable_by_key(remove - 1, |(_, t)| *t);
        for (k, _) in &entries[..remove] {
            let _ = map.remove(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerId {
        // Deterministic distinct peers for tests.
        let kp = libp2p::identity::Keypair::ed25519_from_bytes([n; 32]).expect("valid key");
        kp.public().to_peer_id()
    }

    #[test]
    fn proceeds_then_dedups_same_group_quote() {
        let mut t = TeeAdmissionThrottle::default();
        let now = Instant::now();
        let g = [1u8; 32];
        let q = [2u8; 32];

        // First announce verifies.
        assert!(matches!(t.check(now, peer(1), g, q), Decision::Proceed(_)));
        // Exact replay (same group+quote) is deduped, even from a different
        // peer and a different time within the TTL.
        assert!(matches!(t.check(now, peer(2), g, q), Decision::Duplicate));
        assert!(matches!(
            t.check(now + Duration::from_secs(10), peer(1), g, q),
            Decision::Duplicate
        ));
    }

    #[test]
    fn dedup_is_per_group() {
        let mut t = TeeAdmissionThrottle::default();
        let now = Instant::now();
        let q = [9u8; 32];
        assert!(matches!(
            t.check(now, peer(1), [1u8; 32], q),
            Decision::Proceed(_)
        ));
        // Same quote, different group: not a duplicate.
        assert!(matches!(
            t.check(now, peer(1), [2u8; 32], q),
            Decision::Proceed(_)
        ));
    }

    #[test]
    fn dedup_expires_after_ttl() {
        let mut t =
            TeeAdmissionThrottle::new(8, 100.0, Duration::from_secs(1), Duration::from_secs(60));
        let now = Instant::now();
        let g = [1u8; 32];
        let q = [2u8; 32];
        assert!(matches!(t.check(now, peer(1), g, q), Decision::Proceed(_)));
        assert!(matches!(
            t.check(now + Duration::from_secs(30), peer(1), g, q),
            Decision::Duplicate
        ));
        // After the TTL the same quote is allowed through again.
        assert!(matches!(
            t.check(now + Duration::from_secs(61), peer(1), g, q),
            Decision::Proceed(_)
        ));
    }

    #[test]
    fn per_peer_rate_limit_bursts_then_throttles() {
        // Burst 3, refill 1 token/sec. Big inflight cap so capacity isn't the
        // gate. Each announce uses a distinct quote so dedup isn't the gate.
        let mut t =
            TeeAdmissionThrottle::new(1000, 3.0, Duration::from_secs(1), Duration::from_secs(300));
        let now = Instant::now();
        let g = [1u8; 32];
        let p = peer(1);

        for i in 0..3u8 {
            assert!(
                matches!(t.check(now, p, g, [i; 32]), Decision::Proceed(_)),
                "burst token {i} should pass"
            );
        }
        // 4th within the same instant: bucket empty.
        assert!(matches!(
            t.check(now, p, g, [100u8; 32]),
            Decision::RateLimited
        ));

        // A different peer is unaffected (per-peer bucket).
        assert!(matches!(
            t.check(now, peer(2), g, [101u8; 32]),
            Decision::Proceed(_)
        ));

        // After 1s, one token refills.
        assert!(matches!(
            t.check(now + Duration::from_secs(1), p, g, [102u8; 32]),
            Decision::Proceed(_)
        ));
        assert!(matches!(
            t.check(now + Duration::from_secs(1), p, g, [103u8; 32]),
            Decision::RateLimited
        ));
    }

    #[test]
    fn rate_limited_announce_does_not_consume_token_or_dedup() {
        let mut t = TeeAdmissionThrottle::new(
            1000,
            1.0,
            Duration::from_secs(1000),
            Duration::from_secs(300),
        );
        let now = Instant::now();
        let g = [1u8; 32];
        let p = peer(1);
        // Use the single burst token.
        assert!(matches!(
            t.check(now, p, g, [1u8; 32]),
            Decision::Proceed(_)
        ));
        // Now rate-limited for quote [2;32].
        assert!(matches!(
            t.check(now, p, g, [2u8; 32]),
            Decision::RateLimited
        ));
        // That rejected quote was NOT recorded for dedup: once a token is
        // available again it can proceed (proving no dedup side effect).
        assert!(matches!(
            t.check(now + Duration::from_secs(1000), p, g, [2u8; 32]),
            Decision::Proceed(_)
        ));
    }

    #[test]
    fn rejections_do_not_slow_token_recovery() {
        // Regression guard: each `check` advances `bucket.last = now` *after*
        // crediting the elapsed refill, so a rejection between two accepts must
        // not steal recovery credit. Burst 1, refill 1 token/sec.
        let mut t =
            TeeAdmissionThrottle::new(1000, 1.0, Duration::from_secs(1), Duration::from_secs(300));
        let t0 = Instant::now();
        let g = [1u8; 32];
        let p = peer(1);

        // Spend the single token.
        assert!(matches!(t.check(t0, p, g, [0u8; 32]), Decision::Proceed(_)));
        // Half a second in: only 0.5 token refilled → rejected. The bucket is
        // committed only on Proceed, so this rejection does NOT advance `last`;
        // the full 1s of elapsed time is therefore credited at t0+1s.
        assert!(matches!(
            t.check(t0 + Duration::from_millis(500), p, g, [1u8; 32]),
            Decision::RateLimited
        ));
        // At exactly t0+1s the bucket must be back to a full token despite the
        // intervening rejection — recovery tracks wall-clock, not call count.
        assert!(matches!(
            t.check(t0 + Duration::from_secs(1), p, g, [2u8; 32]),
            Decision::Proceed(_)
        ));
    }

    #[test]
    fn refused_new_peer_is_not_tracked() {
        // Inflight cap 1; hold the only permit so a second, brand-new peer can
        // only ever hit AtCapacity. Generous burst/refill so Gate 2 never bites.
        let mut t = TeeAdmissionThrottle::new(
            1,
            1000.0,
            Duration::from_millis(1),
            Duration::from_secs(300),
        );
        let now = Instant::now();
        let g = [1u8; 32];
        let _held = match t.check(now, peer(1), g, [1u8; 32]) {
            Decision::Proceed(p) => p,
            other => panic!("expected Proceed, got {other:?}"),
        };
        assert_eq!(t.peers.len(), 1, "the proceeding peer is tracked");
        // A new peer that is refused at Gate 3 must not be inserted, so a flood
        // of unique never-proceeding peers can't grow the map.
        assert!(matches!(
            t.check(now, peer(2), g, [2u8; 32]),
            Decision::AtCapacity
        ));
        assert_eq!(t.peers.len(), 1, "a refused new peer must not grow the map");
    }

    #[test]
    fn idle_prune_drops_long_idle_peer_keeps_recently_seen() {
        // burst 5, refill 1 token/sec → full-refill window = 5 * 1s = 5s.
        let mut t =
            TeeAdmissionThrottle::new(1000, 5.0, Duration::from_secs(1), Duration::from_secs(300));
        let now = Instant::now();
        let g = [0u8; 32];

        // Peer 1: seen once at t0, then never again.
        assert!(matches!(
            t.check(now, peer(1), g, [1u8; 32]),
            Decision::Proceed(_)
        ));
        // Peer 2: seen at t0, and again just before the prune so `last_seen` is
        // recent (even though its bucket has also refilled by wall-clock).
        assert!(matches!(
            t.check(now, peer(2), g, [2u8; 32]),
            Decision::Proceed(_)
        ));
        let _ = t.check(now + Duration::from_millis(4500), peer(2), g, [3u8; 32]);

        // Prune at t0+6s (> the 5s full-refill window). Peer 1 has been idle 6s
        // (≥ window) → pruned; peer 2 was seen 1.5s ago (< window) → retained.
        t.prune(now + Duration::from_secs(6));
        assert!(
            !t.peers.contains_key(&peer(1)),
            "peer idle past the full-refill window should be pruned"
        );
        assert!(
            t.peers.contains_key(&peer(2)),
            "recently-seen peer must be retained regardless of bucket level"
        );
    }

    #[test]
    fn forget_quote_allows_reverify_after_transient_failure() {
        // Generous burst/inflight so only the dedup gate is in play.
        let mut t = TeeAdmissionThrottle::new(
            1000,
            100.0,
            Duration::from_secs(1),
            Duration::from_secs(300),
        );
        let now = Instant::now();
        let g = [1u8; 32];
        let q = [2u8; 32];

        // First announce proceeds and records the dedup entry.
        assert!(matches!(t.check(now, peer(1), g, q), Decision::Proceed(_)));
        // A re-announce of the same quote within the TTL is deduped.
        assert!(matches!(t.check(now, peer(1), g, q), Decision::Duplicate));
        // The verify errored transiently → forget the entry.
        t.forget_quote(g, q);
        // The same quote may now be re-verified (retry resilience restored).
        assert!(matches!(t.check(now, peer(1), g, q), Decision::Proceed(_)));
    }

    #[test]
    fn global_inflight_cap_blocks_when_saturated() {
        // Cap of 2 concurrent verifies; generous per-peer + dedup so this is
        // the only gate. Hold the returned permits to simulate in-flight work.
        let mut t = TeeAdmissionThrottle::new(
            2,
            1000.0,
            Duration::from_millis(1),
            Duration::from_secs(300),
        );
        let now = Instant::now();
        let g = [1u8; 32];

        let p1 = match t.check(now, peer(1), g, [1u8; 32]) {
            Decision::Proceed(p) => p,
            other => panic!("expected Proceed, got {other:?}"),
        };
        let _p2 = match t.check(now, peer(2), g, [2u8; 32]) {
            Decision::Proceed(p) => p,
            other => panic!("expected Proceed, got {other:?}"),
        };
        // Both permits held → at capacity.
        assert!(matches!(
            t.check(now, peer(3), g, [3u8; 32]),
            Decision::AtCapacity
        ));

        // Completing one verify (drop its permit) frees a slot.
        drop(p1);
        assert!(matches!(
            t.check(now, peer(4), g, [4u8; 32]),
            Decision::Proceed(_)
        ));
    }

    #[test]
    fn at_capacity_does_not_consume_token_or_dedup() {
        let mut t = TeeAdmissionThrottle::new(
            1,
            1000.0,
            Duration::from_millis(1),
            Duration::from_secs(300),
        );
        let now = Instant::now();
        let g = [1u8; 32];
        let p = peer(1);
        let held = match t.check(now, p, g, [7u8; 32]) {
            Decision::Proceed(perm) => perm,
            other => panic!("expected Proceed, got {other:?}"),
        };
        // Saturated.
        assert!(matches!(
            t.check(now, p, g, [8u8; 32]),
            Decision::AtCapacity
        ));
        drop(held);
        // The capacity-rejected quote [8;32] was not recorded for dedup, and
        // the token was not burned: it proceeds once a slot is free.
        assert!(matches!(
            t.check(now, p, g, [8u8; 32]),
            Decision::Proceed(_)
        ));
    }
}
