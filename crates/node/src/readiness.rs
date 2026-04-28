//! Per-namespace readiness FSM, beacon cache, and (in later tasks)
//! the actor that emits beacons and handles probes.
//!
//! Phase 6 of the three-phase governance contract: pure types + logic.
//! The actor wiring (periodic beacon emission, probe handling) lands in
//! Phase 7; the join-flow consumer (`await_first_fresh_beacon`,
//! `join_namespace`, `await_namespace_ready`) lands in Phase 8.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use calimero_context_client::local_governance::SignedReadinessBeacon;
use calimero_primitives::identity::PublicKey;

#[cfg(test)]
mod tests;

/// Tier in the per-namespace readiness FSM.
///
/// Data-carrying variants (`CatchingUp { target_applied_through }`,
/// `Degraded { reason }`) keep the FSM, metrics labels, and logs aligned
/// on a single source of truth — a flat enum plus a parallel side-channel
/// struct would risk the variant and the demotion reason drifting apart
/// over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessTier {
    Bootstrapping,
    LocallyReady,
    PeerValidatedReady,
    CatchingUp { target_applied_through: u64 },
    Degraded { reason: DemotionReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemotionReason {
    PendingOps(usize),
    NoRecentPeers,
    PeerSawHigherThroughput,
}

#[derive(Debug, Clone)]
pub struct ReadinessState {
    pub tier: ReadinessTier,
    pub local_applied_through: u64,
    pub local_head: [u8; 32],
    pub local_pending_ops: usize,
    pub subscribed_at: Instant,
}

#[derive(Debug, Clone, Copy)]
pub struct ReadinessConfig {
    pub boot_grace: Duration,
    pub ttl_heartbeat: Duration,
    pub beacon_interval: Duration,
    pub applied_through_grace: u64,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            boot_grace: Duration::from_secs(10),
            ttl_heartbeat: Duration::from_secs(60),
            beacon_interval: Duration::from_secs(5),
            applied_through_grace: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PeerSummary {
    pub max_applied_through: Option<u64>,
    pub heard_recent_beacon: bool,
}

/// Pure transition function for the readiness FSM.
///
/// Maps `(state, peers, cfg, now)` → next `ReadinessTier`. The function
/// is total (every input combination has a defined output) and free of
/// side effects; the actor in Phase 7 calls it on every beacon, every
/// freshness tick, and on local-state changes.
pub fn evaluate_readiness(
    state: &ReadinessState,
    peers: &PeerSummary,
    cfg: &ReadinessConfig,
    now: Instant,
) -> ReadinessTier {
    // Pending ops always demote — record the count so observability can see
    // *how many* ops are blocking promotion, not just that *some* exist.
    if state.local_pending_ops > 0 {
        return ReadinessTier::Degraded {
            reason: DemotionReason::PendingOps(state.local_pending_ops),
        };
    }

    // Empty-DAG joiners never self-promote (no LocallyReady from local_applied_through=0).
    // If we hear a peer beacon we know there's a target to catch up to → CatchingUp
    // carrying that target; otherwise we don't know whether a network exists yet →
    // stay Bootstrapping. With the atomic `ReadinessCache::peer_summary` snapshot,
    // `heard_recent_beacon == true` implies `max_applied_through.is_some()`, so the
    // `unwrap_or(0)` is a defensive fallback only.
    if state.local_applied_through == 0 {
        return if peers.heard_recent_beacon {
            ReadinessTier::CatchingUp {
                target_applied_through: peers.max_applied_through.unwrap_or(0),
            }
        } else {
            ReadinessTier::Bootstrapping
        };
    }

    let boot_grace_elapsed = now.duration_since(state.subscribed_at) >= cfg.boot_grace;

    match (
        peers.max_applied_through,
        peers.heard_recent_beacon,
        boot_grace_elapsed,
    ) {
        // Heard a peer beacon: tip-fresh → PeerValidatedReady; behind → CatchingUp{target}.
        (Some(peer_at), true, _) => {
            if state.local_applied_through + cfg.applied_through_grace >= peer_at {
                ReadinessTier::PeerValidatedReady
            } else {
                ReadinessTier::CatchingUp {
                    target_applied_through: peer_at,
                }
            }
        }
        // No peer beacons but we've waited BOOT_GRACE: self-promote (LocallyReady).
        (None, false, true) => ReadinessTier::LocallyReady,
        // No peer beacons and still in boot grace: stay Bootstrapping.
        (None, false, false) => ReadinessTier::Bootstrapping,
        // Defensive: with an atomic `ReadinessCache::peer_summary` snapshot, both
        // `(None, true, _)` and `(Some(_), false, _)` are unreachable —
        // `max_applied_through` and `heard_recent_beacon` are both derived from
        // the same fresh-within-TTL filter, so they are always either
        // (None, false) or (Some(_), true). The arms below remain as safe
        // fallbacks for any future non-atomic call site, return spec
        // §7.2-aligned tiers, and `debug_assert!` loud in dev builds so a
        // regression is caught immediately.
        //
        // `(None, true)`: claim of fresh peer with no max_applied_through →
        // no usable target → stay Bootstrapping (no self-promotion).
        (None, true, _) => {
            debug_assert!(
                false,
                "PeerSummary built from non-atomic reads (None, true) — use ReadinessCache::peer_summary"
            );
            ReadinessTier::Bootstrapping
        }
        // `(Some(_), false)`: we knew about a peer once, no fresh beacon now.
        // Spec §7.2 says `*Ready → Degraded { reason: NoRecentPeers }`.
        (Some(_), false, _) => {
            debug_assert!(
                false,
                "PeerSummary built from non-atomic reads (Some, false) — use ReadinessCache::peer_summary"
            );
            ReadinessTier::Degraded {
                reason: DemotionReason::NoRecentPeers,
            }
        }
    }
}

/// Per-(namespace, peer) snapshot of the most recent fresh beacon we
/// have received from that peer.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub head: [u8; 32],
    pub applied_through: u64,
    /// Peer-signed millis-since-epoch from the beacon itself.
    /// Authoritative per-peer ordering signal — used by `insert` to drop
    /// stale beacons that gossipsub may re-deliver out-of-order on mesh
    /// churn / peer reconnect.
    pub ts_millis: u64,
    pub received_at: Instant,
    pub strong: bool,
}

/// Maximum tolerated drift between a beacon's `ts_millis` and local
/// wall-clock. Beacons claiming a wall-clock more than this far in the
/// future are rejected to close the cache-poisoning vector documented
/// on [`ReadinessCache::insert`].
///
/// 60s tolerates legitimate NTP-synced clock drift while bounding the
/// damage a malicious or badly-skewed signer can do.
pub const MAX_BEACON_CLOCK_DRIFT_MS: u64 = 60_000;

/// Per-namespace, per-peer beacon cache.
///
/// Uses `BTreeMap` (not `HashMap`) because `calimero_primitives::identity::PublicKey`
/// derives `Ord` but not `Hash`. Lookups are O(log n) on a per-namespace
/// map that holds at most one entry per peer; the practical n is the
/// namespace member count, well within a regime where the constant
/// factors of `BTreeMap` are competitive with `HashMap`.
#[derive(Default)]
pub struct ReadinessCache {
    entries: Mutex<BTreeMap<([u8; 32], PublicKey), CacheEntry>>,
}

impl ReadinessCache {
    /// Insert iff the incoming beacon is *newer* than any cached entry from
    /// the same peer (by `ts_millis`, with `applied_through` as tiebreaker
    /// on clock equality). Gossipsub does not guarantee delivery order —
    /// without this filter, an older re-delivered beacon could overwrite a
    /// fresher one, causing `pick_sync_partner` and `peer_summary` to
    /// regress and the FSM to spuriously demote
    /// `PeerValidatedReady → CatchingUp`.
    ///
    /// Also rejects beacons with `ts_millis` more than
    /// [`MAX_BEACON_CLOCK_DRIFT_MS`] ahead of local wall-clock. Without
    /// this bound, a malicious or clock-skewed member could sign a beacon
    /// with `ts_millis = year 2100`, poisoning their cache entry: every
    /// subsequent legitimate beacon from the same peer would be dropped
    /// by the `older-than-existing` filter, freezing `applied_through`
    /// and `dag_head` at attacker-chosen values indefinitely. Beacons
    /// are signed and verified against namespace membership, so only
    /// current members can attempt this — but a single compromised key
    /// would otherwise be sufficient.
    pub fn insert(&self, beacon: &SignedReadinessBeacon) {
        // Wall-clock sanity bound — reject far-future ts_millis to close
        // the cache-poisoning attack described above.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        if beacon.ts_millis > now_ms.saturating_add(MAX_BEACON_CLOCK_DRIFT_MS) {
            return;
        }

        let mut g = self.entries.lock().expect("readiness cache lock");
        let key = (beacon.namespace_id, beacon.peer_pubkey);
        if let Some(existing) = g.get(&key) {
            // Drop the beacon if it's older or equal-clock-but-not-fresher.
            if beacon.ts_millis < existing.ts_millis
                || (beacon.ts_millis == existing.ts_millis
                    && beacon.applied_through <= existing.applied_through)
            {
                return;
            }
        }
        let _ = g.insert(
            key,
            CacheEntry {
                head: beacon.dag_head,
                applied_through: beacon.applied_through,
                ts_millis: beacon.ts_millis,
                received_at: Instant::now(),
                strong: beacon.strong,
            },
        );
    }

    pub fn fresh_peers(&self, ns: [u8; 32], ttl: Duration) -> Vec<(PublicKey, CacheEntry)> {
        let g = self.entries.lock().expect("readiness cache lock");
        let now = Instant::now();
        g.iter()
            .filter(|((nns, _), e)| *nns == ns && now.duration_since(e.received_at) <= ttl)
            .map(|((_, pk), e)| (*pk, e.clone()))
            .collect()
    }

    /// Sort order: `(strong desc, applied_through desc, received_at desc)`.
    pub fn pick_sync_partner(
        &self,
        ns: [u8; 32],
        ttl: Duration,
    ) -> Option<(PublicKey, CacheEntry)> {
        let mut peers = self.fresh_peers(ns, ttl);
        peers.sort_by(|a, b| {
            b.1.strong
                .cmp(&a.1.strong)
                .then(b.1.applied_through.cmp(&a.1.applied_through))
                .then(b.1.received_at.cmp(&a.1.received_at))
        });
        peers.into_iter().next()
    }

    pub fn max_applied_through(&self, ns: [u8; 32], ttl: Duration) -> Option<u64> {
        self.fresh_peers(ns, ttl)
            .into_iter()
            .map(|(_, e)| e.applied_through)
            .max()
    }

    /// Atomic snapshot — `max_applied_through` and `heard_recent_beacon`
    /// are read under a single lock acquisition so the FSM's match arms
    /// cannot observe a torn state (e.g. `heard_recent_beacon=true`
    /// while `max_applied_through=None`). All call sites that build a
    /// `PeerSummary` MUST use this rather than two separate calls to
    /// `max_applied_through` and `fresh_peers`.
    pub fn peer_summary(&self, ns: [u8; 32], ttl: Duration) -> PeerSummary {
        let g = self.entries.lock().expect("readiness cache lock");
        let now = Instant::now();
        let mut max_applied: Option<u64> = None;
        let mut any_fresh = false;
        for ((nns, _), e) in g.iter() {
            if *nns != ns || now.duration_since(e.received_at) > ttl {
                continue;
            }
            any_fresh = true;
            max_applied = Some(max_applied.map_or(e.applied_through, |m| m.max(e.applied_through)));
        }
        PeerSummary {
            max_applied_through: max_applied,
            heard_recent_beacon: any_fresh,
        }
    }
}
