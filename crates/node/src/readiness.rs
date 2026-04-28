//! Per-namespace readiness FSM, beacon cache, and (in later tasks)
//! the actor that emits beacons and handles probes.
//!
//! Phase 6 of the three-phase governance contract: pure types + logic.
//! The actor wiring (periodic beacon emission, probe handling) lands in
//! Phase 7; the join-flow consumer (`await_first_fresh_beacon`,
//! `join_namespace`, `await_namespace_ready`) lands in Phase 8.

use std::time::{Duration, Instant};

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
