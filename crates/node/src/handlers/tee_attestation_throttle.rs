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
    last: Instant,
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
    pub fn new(
        max_inflight: usize,
        per_peer_burst: f64,
        per_peer_refill: Duration,
        dedup_ttl: Duration,
    ) -> Self {
        assert!(max_inflight > 0, "max_inflight must be positive");
        assert!(per_peer_burst >= 1.0, "per_peer_burst must be >= 1");
        Self {
            inflight: Arc::new(Semaphore::new(max_inflight)),
            peers: HashMap::new(),
            recent_quotes: HashMap::new(),
            per_peer_burst,
            per_peer_refill,
            dedup_ttl,
        }
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
        self.prune(now);

        // Gate 1: per-group quote dedup. Cheapest, and the most effective
        // guard against single-quote replay floods.
        let key = (group_id, quote_hash);
        if let Some(seen) = self.recent_quotes.get(&key) {
            if now.duration_since(*seen) < self.dedup_ttl {
                return Decision::Duplicate;
            }
        }

        // Gate 2: per-peer rate limit. Compute available tokens lazily from
        // elapsed time; do NOT consume yet (a later gate may still reject).
        let burst = self.per_peer_burst;
        let refill_per_sec = if self.per_peer_refill.as_secs_f64() > 0.0 {
            1.0 / self.per_peer_refill.as_secs_f64()
        } else {
            f64::INFINITY
        };
        let bucket = self.peers.entry(source).or_insert(PeerBucket {
            tokens: burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * refill_per_sec).min(burst);
        bucket.last = now;
        if bucket.tokens < 1.0 {
            return Decision::RateLimited;
        }

        // Gate 3: global inflight cap. Acquire last so a rejection here does
        // not burn a per-peer token.
        let permit = match Arc::clone(&self.inflight).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return Decision::AtCapacity,
        };

        // All gates passed: commit the side effects.
        bucket.tokens -= 1.0;
        let _ = self.recent_quotes.insert(key, now);
        Decision::Proceed(permit)
    }

    /// Drop expired dedup entries and full/idle peer buckets, and hard-cap map
    /// sizes so adversarial churn can't grow memory without bound.
    fn prune(&mut self, now: Instant) {
        let dedup_ttl = self.dedup_ttl;
        self.recent_quotes
            .retain(|_, seen| now.duration_since(*seen) < dedup_ttl);
        if self.recent_quotes.len() > MAX_TRACKED_QUOTES {
            Self::evict_oldest(&mut self.recent_quotes, MAX_TRACKED_QUOTES);
        }

        // A peer whose bucket has refilled to full and hasn't been seen for a
        // while carries no state worth keeping.
        let burst = self.per_peer_burst;
        let idle_cutoff = self.per_peer_refill.saturating_mul(2);
        self.peers.retain(|_, b| {
            !(b.tokens >= burst && now.saturating_duration_since(b.last) > idle_cutoff)
        });
        if self.peers.len() > MAX_TRACKED_PEERS {
            Self::evict_oldest_peers(&mut self.peers, MAX_TRACKED_PEERS);
        }
    }

    fn evict_oldest<K: Clone + std::hash::Hash + Eq>(map: &mut HashMap<K, Instant>, keep: usize) {
        let mut entries: Vec<(K, Instant)> = map.iter().map(|(k, v)| (k.clone(), *v)).collect();
        entries.sort_by_key(|(_, t)| *t);
        for (k, _) in entries.into_iter().take(map.len().saturating_sub(keep)) {
            let _ = map.remove(&k);
        }
    }

    fn evict_oldest_peers(map: &mut HashMap<PeerId, PeerBucket>, keep: usize) {
        let mut entries: Vec<(PeerId, Instant)> = map.iter().map(|(k, v)| (*k, v.last)).collect();
        entries.sort_by_key(|(_, t)| *t);
        for (k, _) in entries.into_iter().take(map.len().saturating_sub(keep)) {
            let _ = map.remove(&k);
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
