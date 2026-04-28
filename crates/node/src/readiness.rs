//! Per-namespace readiness FSM, beacon cache, and the [`ReadinessManager`]
//! actor that emits beacons and handles probes.
//!
//! Implements implementation-plan Phase 6 (FSM + cache types) and
//! Phase 7 (actor / emission / probe handling) of the three-phase
//! governance contract for #2237. The "three phases" referred to are
//! `assert_transport_ready` / `publish + collect acks` / `apply on
//! receipt` — see `crates/context/src/governance_broadcast.rs`. The
//! "Phase 6/7/8" numbers in this PR refer to the implementation plan
//! at `docs/superpowers/plans/2026-04-25-governance-three-phase-readiness.md`,
//! which slices the work into landable chunks.
//!
//! The join-flow consumer (`await_first_fresh_beacon` via
//! [`ReadinessCache::await_first_fresh_beacon`], plus `join_namespace`
//! / `await_namespace_ready`) lives in Phase 8, partially in this
//! module and partially in [`crate::join_namespace`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use actix::{Actor, AsyncContext, Context, Handler, Message};
use calimero_context_client::local_governance::SignedReadinessBeacon;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use libp2p::PeerId;
use zeroize::Zeroize;

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
    /// We had a fresh peer beacon for this namespace once, but no
    /// peer has emitted within `ttl_heartbeat` recently — the spec
    /// §7.2 "*Ready → Degraded" arm. Surfaced from `evaluate_readiness`
    /// when `peer_summary` returns the (defensive) `(Some, false)`
    /// state that should be unreachable under atomic-snapshot reads.
    NoRecentPeers,
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
            // saturating_add: in debug builds an overflow on
            // `local_applied_through + applied_through_grace` would
            // panic if `local_applied_through` were near `u64::MAX` —
            // an unreachable state in practice, but a defensive
            // saturating_add costs nothing and silences the audit.
            if state
                .local_applied_through
                .saturating_add(cfg.applied_through_grace)
                >= peer_at
            {
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
#[derive(Debug, Default)]
pub struct ReadinessCache {
    entries: Mutex<BTreeMap<([u8; 32], PublicKey), CacheEntry>>,
}

impl ReadinessCache {
    /// Acquire the entries map, recovering from a poisoned mutex.
    ///
    /// A `PoisonError` only happens if a previous holder panicked while
    /// the guard was live; the BTreeMap's invariants are not at risk
    /// here (no nested invariants between entries), so continuing with
    /// the inner guard via `into_inner()` is strictly preferable to
    /// permanently DoSing the readiness subsystem on the first transient
    /// panic. Mirrors `AckRouter::lock` from PR #2264.
    fn entries_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, BTreeMap<([u8; 32], PublicKey), CacheEntry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ReadinessCache {
    /// Insert a beacon into the cache.
    ///
    /// **Verification contract**: this method assumes the beacon has
    /// already been verified for signature AND namespace membership by
    /// the caller. The single legitimate caller is the receiver-side
    /// `network_event::readiness::handle_readiness_beacon`, which calls
    /// `calimero_context::governance_broadcast::verify_readiness_beacon`
    /// (sig + member-set check) BEFORE invoking `insert`. Putting
    /// verification inside `insert` would couple the cache to
    /// `&Store`, drag namespace-membership state into a pure-types
    /// module, and duplicate work since the receiver gate runs first.
    ///
    /// Insert iff the incoming beacon is *newer* than any cached entry
    /// from the same peer (by `ts_millis`, with `applied_through` as
    /// tiebreaker on clock equality). Gossipsub does not guarantee
    /// delivery order — without this filter, an older re-delivered
    /// beacon could overwrite a fresher one, causing `pick_sync_partner`
    /// and `peer_summary` to regress and the FSM to spuriously demote
    /// `PeerValidatedReady → CatchingUp`.
    ///
    /// Also rejects beacons with `ts_millis` more than
    /// [`MAX_BEACON_CLOCK_DRIFT_MS`] ahead of local wall-clock. Without
    /// this bound, a malicious or clock-skewed member could sign a beacon
    /// with `ts_millis = year 2100`, poisoning their cache entry: every
    /// subsequent legitimate beacon from the same peer would be dropped
    /// by the `older-than-existing` filter, freezing `applied_through`
    /// and `dag_head` at attacker-chosen values indefinitely.
    ///
    /// Opportunistically evicts entries past `2 × MAX_BEACON_CLOCK_DRIFT_MS`
    /// for *this namespace* on every insert — keeps long-lived nodes
    /// from accumulating entries from peers that left the namespace.
    /// Stale-but-within-eviction-window entries are still filtered out
    /// of `fresh_peers`/`peer_summary` by the per-call `ttl` check.
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

        let now = Instant::now();
        let mut g = self.entries_lock();
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

        // Opportunistic eviction for the same namespace — keep the
        // BTreeMap from accumulating entries from peers that left the
        // namespace on long-running nodes. Eviction window
        // (`2 × MAX_BEACON_CLOCK_DRIFT_MS`) is intentionally wider
        // than typical TTLs so reads can still see "stale-but-recent"
        // entries (filtered by per-call `ttl`) without competing
        // against this prune.
        let evict_window = Duration::from_millis(MAX_BEACON_CLOCK_DRIFT_MS.saturating_mul(2));
        g.retain(|(ns, _), entry| {
            *ns != beacon.namespace_id || now.duration_since(entry.received_at) <= evict_window
        });

        let _ = g.insert(
            key,
            CacheEntry {
                head: beacon.dag_head,
                applied_through: beacon.applied_through,
                ts_millis: beacon.ts_millis,
                received_at: now,
                strong: beacon.strong,
            },
        );
    }

    pub fn fresh_peers(&self, ns: [u8; 32], ttl: Duration) -> Vec<(PublicKey, CacheEntry)> {
        let g = self.entries_lock();
        let now = Instant::now();
        g.iter()
            .filter(|((nns, _), e)| *nns == ns && now.duration_since(e.received_at) <= ttl)
            .map(|((_, pk), e)| (*pk, e.clone()))
            .collect()
    }

    /// Sort order: `(strong desc, applied_through desc, received_at desc)`.
    ///
    /// O(n) via `Iterator::max_by` — earlier sort-then-take-first was
    /// O(n log n) for a single-element selection.
    pub fn pick_sync_partner(
        &self,
        ns: [u8; 32],
        ttl: Duration,
    ) -> Option<(PublicKey, CacheEntry)> {
        self.fresh_peers(ns, ttl).into_iter().max_by(|a, b| {
            a.1.strong
                .cmp(&b.1.strong)
                .then(a.1.applied_through.cmp(&b.1.applied_through))
                .then(a.1.received_at.cmp(&b.1.received_at))
        })
    }

    /// Atomic snapshot — `max_applied_through` and `heard_recent_beacon`
    /// are read under a single lock acquisition so the FSM's match arms
    /// cannot observe a torn state (e.g. `heard_recent_beacon=true`
    /// while `max_applied_through=None`). All call sites that build a
    /// `PeerSummary` MUST use this rather than two separate calls to
    /// `max_applied_through` and `fresh_peers`.
    pub fn peer_summary(&self, ns: [u8; 32], ttl: Duration) -> PeerSummary {
        let g = self.entries_lock();
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

/// Per-namespace beacon emitter and FSM driver.
///
/// Holds:
/// - the shared [`ReadinessCache`] so the receiver-side handler can
///   `cache.insert(&beacon)` directly without an actor-mailbox hop
///   (the cache is internally synchronised),
/// - the [`NodeClient`] for `publish` access on the namespace topic
///   (bypassing the 10s mesh-wait gate of `publish_on_namespace` —
///   beacon emission is best-effort and must not block the periodic
///   tick),
/// - the [`Store`] for namespace-identity loading in beacon signing
///   (Task 7.2),
/// - per-namespace local FSM state and last-probe-response timestamps
///   for the `BEACON_INTERVAL/2` rate limit (Task 7.3).
pub struct ReadinessManager {
    pub cache: Arc<ReadinessCache>,
    pub config: ReadinessConfig,
    pub state_per_namespace: HashMap<[u8; 32], ReadinessState>,
    pub node_client: NodeClient,
    pub datastore: Store,
    /// Per-(peer, namespace) timestamp of the last out-of-cycle beacon
    /// emitted in response to a [`ReadinessProbe`]. Used by Task 7.3 to
    /// rate-limit probe responses at `BEACON_INTERVAL / 2` and close the
    /// unsigned-probe traffic-amplification path.
    pub last_probe_response_at: HashMap<(PeerId, [u8; 32]), Instant>,
}

impl Actor for ReadinessManager {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // Periodic freshness-tick beacon emission. Only namespaces in a
        // *Ready tier emit; the filter is inside `emit_periodic_beacons`.
        ctx.run_interval(self.config.beacon_interval, |this, _ctx| {
            this.emit_periodic_beacons();
        });
    }
}

/// Hint that a peer beacon has just been inserted into the cache for this
/// namespace and the FSM should be re-evaluated. Sent by the receiver-side
/// `handle_readiness_beacon` handler in Task 7.3.
#[derive(Message)]
#[rtype(result = "()")]
pub struct ApplyBeaconLocal {
    pub namespace_id: [u8; 32],
}

/// Carries a snapshot of locally-observed namespace state into the FSM
/// driver. The actor merges this into `state_per_namespace`, re-evaluates
/// `evaluate_readiness`, and emits an edge-trigger beacon if the tier
/// transitions into a *Ready variant.
#[derive(Message)]
#[rtype(result = "()")]
pub struct LocalStateChanged {
    pub namespace_id: [u8; 32],
    pub local_applied_through: u64,
    pub local_head: [u8; 32],
    pub local_pending_ops: usize,
}

/// A single namespace governance op was successfully applied locally.
///
/// Sent from the receiver-side network-event handler after a
/// `NamespaceApplyOutcome::Applied`. The actor increments its own
/// per-namespace `local_applied_through` counter — this is the single
/// progress signal the FSM needs to leave `Bootstrapping` and start
/// emitting beacons.
///
/// The actor maintains the count internally (not the caller) because
/// the count exists nowhere else in the codebase: `apply_signed_namespace_op`
/// doesn't return a new height, and the namespace DAG doesn't expose a
/// monotonic apply counter. Keeping the count on the actor keeps the
/// scope minimal.
#[derive(Message)]
#[rtype(result = "()")]
pub struct NamespaceOpApplied {
    pub namespace_id: [u8; 32],
}

impl ReadinessManager {
    fn emit_periodic_beacons(&mut self) {
        // Snapshot the (ns_id, state) pairs we want to publish so the
        // borrow on `self.state_per_namespace` is released before
        // `publish_beacon` runs (`publish_beacon` will load identity
        // material from `self.datastore` in Task 7.2).
        let to_emit: Vec<([u8; 32], ReadinessState)> = self
            .state_per_namespace
            .iter()
            .filter(|(_, s)| {
                matches!(
                    s.tier,
                    ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady
                )
            })
            .map(|(ns, s)| (*ns, s.clone()))
            .collect();
        for (ns_id, state) in to_emit {
            self.publish_beacon(ns_id, &state);
        }
    }

    /// Sign and publish a [`SignedReadinessBeacon`] on the namespace
    /// topic.
    ///
    /// Best-effort: any error is logged at `debug` (no peers subscribed
    /// yet, identity not yet provisioned, etc.) and the call returns
    /// silently. The freshness-tick interval will retry on the next
    /// `beacon_interval`, and an edge-trigger beacon will fire on the
    /// next tier transition into `*Ready`.
    ///
    /// The signed body uses [`SignedReadinessBeacon::signable_bytes`] —
    /// the canonical scheme defined alongside the wire type in
    /// `calimero_context_client::local_governance::wire`. Receivers
    /// verify via [`SignedReadinessBeacon::verify_signature`] which
    /// includes the `READINESS_BEACON_SIGN_DOMAIN` prefix and rejects
    /// field-substitution replays (proven by the tamper tests in
    /// that module).
    fn publish_beacon(&self, ns_id: [u8; 32], state: &ReadinessState) {
        use calimero_context_client::local_governance::{NamespaceTopicMsg, SignedReadinessBeacon};
        use calimero_node_primitives::sync::BroadcastMessage;

        let group_id = calimero_context_config::types::ContextGroupId::from(ns_id);
        let identity =
            match calimero_context::group_store::get_namespace_identity(&self.datastore, &group_id)
            {
                Ok(Some(id)) => id,
                Ok(None) => return, // No identity for this namespace yet — skip.
                Err(err) => {
                    tracing::debug!(?err, ?ns_id, "ReadinessBeacon: identity load failed");
                    return;
                }
            };
        let (peer_pubkey, mut sk_bytes, mut sender_key) = identity;
        // `sender_key` is unused on the beacon path — zeroize immediately.
        // `sk_bytes` is consumed into `PrivateKey::from(...)` below;
        // because `[u8; 32]: Copy`, that "move" actually leaves a copy
        // of the bytes on the stack here, so we explicitly zeroize the
        // local AFTER the signing block. `PrivateKey`'s `Drop` impl
        // zeroizes its own internal copy.
        sender_key.zeroize();

        let strong = matches!(state.tier, ReadinessTier::PeerValidatedReady);
        let ts_millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Build with a placeholder signature, sign over the canonical
        // signable_bytes(), then write the real signature back.
        let mut beacon = SignedReadinessBeacon {
            namespace_id: ns_id,
            peer_pubkey,
            dag_head: state.local_head,
            applied_through: state.local_applied_through,
            ts_millis,
            strong,
            signature: [0u8; 64],
        };
        let signable = match beacon.signable_bytes() {
            Ok(s) => s,
            Err(err) => {
                tracing::debug!(?err, "ReadinessBeacon: signable_bytes failed");
                return;
            }
        };
        let signing_key = calimero_primitives::identity::PrivateKey::from(sk_bytes);
        // Wipe the stack copy that `Copy`-move-into-PrivateKey left
        // behind. `signing_key` itself is dropped at the end of the
        // function and zeroizes via its own `Drop` impl.
        sk_bytes.zeroize();
        let signature = match signing_key.sign(&signable) {
            Ok(sig) => sig.to_bytes(),
            Err(err) => {
                tracing::debug!(?err, "ReadinessBeacon: sign failed");
                return;
            }
        };
        beacon.signature = signature;

        let topic = calimero_context::governance_broadcast::ns_topic(ns_id);
        // Wrap the NamespaceTopicMsg in the BroadcastMessage envelope used
        // on `ns/<id>` topics — the receiver-side dispatch in
        // `network_event::handle_namespace_governance_delta` unwraps
        // NamespaceGovernanceDelta and decodes the inner NamespaceTopicMsg.
        // delta_id/parent_ids are zero/empty since beacons are not DAG content.
        let inner = match borsh::to_vec(&NamespaceTopicMsg::ReadinessBeacon(beacon)) {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(?err, "ReadinessBeacon: borsh encode (inner) failed");
                return;
            }
        };
        let envelope = BroadcastMessage::NamespaceGovernanceDelta {
            namespace_id: ns_id,
            delta_id: [0u8; 32],
            parent_ids: Vec::new(),
            payload: inner,
        };
        let bytes = match borsh::to_vec(&envelope) {
            Ok(b) => b,
            Err(err) => {
                tracing::debug!(?err, "ReadinessBeacon: borsh encode (envelope) failed");
                return;
            }
        };

        // Detached publish — the caller (`emit_periodic_beacons` /
        // edge-trigger) doesn't await; gossipsub publish failures are
        // non-fatal. Using `network_client().publish` directly bypasses
        // the 10s mesh-wait gate of `NodeClient::publish_on_namespace`.
        let net = self.node_client.network_client().clone();
        actix::spawn(async move {
            if let Err(err) = net.publish(topic, bytes).await {
                tracing::debug!(?err, "ReadinessBeacon publish failed (non-fatal)");
            }
        });
    }
}

impl Handler<LocalStateChanged> for ReadinessManager {
    type Result = ();

    fn handle(&mut self, msg: LocalStateChanged, _ctx: &mut Self::Context) {
        // Atomic single-lock snapshot — see ReadinessCache::peer_summary
        // for why peer_summary is the only correct source for PeerSummary.
        let peers = self
            .cache
            .peer_summary(msg.namespace_id, self.config.ttl_heartbeat);

        // Scoped borrow on `state_per_namespace`: compute next tier,
        // mutate the entry, snapshot for emission. The borrow ends at
        // the end of this block so `clear_probe_window_for` and
        // `publish_beacon` can re-borrow `self` afterwards.
        let to_emit = {
            let entry = self
                .state_per_namespace
                .entry(msg.namespace_id)
                .or_insert_with(|| ReadinessState {
                    tier: ReadinessTier::Bootstrapping,
                    local_applied_through: 0,
                    local_head: [0u8; 32],
                    local_pending_ops: 0,
                    subscribed_at: Instant::now(),
                });
            entry.local_applied_through = msg.local_applied_through;
            entry.local_head = msg.local_head;
            entry.local_pending_ops = msg.local_pending_ops;
            let new_tier = evaluate_readiness(entry, &peers, &self.config, Instant::now());
            if new_tier != entry.tier {
                entry.tier = new_tier;
                // Edge trigger: a tier transition into *Ready warrants
                // an immediate beacon so peers see our promotion without
                // waiting for the next freshness tick.
                if matches!(
                    new_tier,
                    ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady
                ) {
                    Some(entry.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(snapshot) = to_emit {
            self.clear_probe_window_for(msg.namespace_id);
            self.publish_beacon(msg.namespace_id, &snapshot);
        }
    }
}

/// Out-of-cycle beacon emission triggered by an inbound
/// [`calimero_context_client::local_governance::ReadinessProbe`].
///
/// Carries the requesting peer so the actor can rate-limit responses
/// per-(peer, namespace) — see [`Handler<EmitOutOfCycleBeacon>`].
#[derive(Message)]
#[rtype(result = "()")]
pub struct EmitOutOfCycleBeacon {
    pub namespace_id: [u8; 32],
    pub requesting_peer: PeerId,
}

impl Handler<NamespaceOpApplied> for ReadinessManager {
    type Result = ();

    fn handle(&mut self, msg: NamespaceOpApplied, _ctx: &mut Self::Context) {
        // Atomic single-lock snapshot — see ReadinessCache::peer_summary
        // for why peer_summary is the only correct source for PeerSummary.
        let peers = self
            .cache
            .peer_summary(msg.namespace_id, self.config.ttl_heartbeat);

        // Increment the per-namespace local apply counter, then re-evaluate
        // FSM and edge-emit on transition into a *Ready tier. Uses the
        // same scoped-borrow pattern as Handler<LocalStateChanged> so
        // `clear_probe_window_for` and `publish_beacon` can re-borrow
        // self after the entry borrow ends.
        let to_emit = {
            let entry = self
                .state_per_namespace
                .entry(msg.namespace_id)
                .or_insert_with(|| ReadinessState {
                    tier: ReadinessTier::Bootstrapping,
                    local_applied_through: 0,
                    local_head: [0u8; 32],
                    local_pending_ops: 0,
                    subscribed_at: Instant::now(),
                });
            entry.local_applied_through = entry.local_applied_through.saturating_add(1);
            let new_tier = evaluate_readiness(entry, &peers, &self.config, Instant::now());
            if new_tier != entry.tier {
                entry.tier = new_tier;
                if matches!(
                    new_tier,
                    ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady
                ) {
                    Some(entry.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(snapshot) = to_emit {
            self.clear_probe_window_for(msg.namespace_id);
            self.publish_beacon(msg.namespace_id, &snapshot);
        }
    }
}

impl Handler<EmitOutOfCycleBeacon> for ReadinessManager {
    type Result = ();

    fn handle(&mut self, msg: EmitOutOfCycleBeacon, _ctx: &mut Self::Context) {
        // Rate-limit probe responses per (peer, namespace) at
        // `BEACON_INTERVAL / 2` to close BOTH:
        // - The traffic-amplification path: one ~48-byte unsigned probe
        //   would otherwise trigger one ~200-byte signed beacon from
        //   EVERY *Ready peer on the topic (≈Nx amplification).
        // - The mailbox-CPU path: even when we drop on tier (non-Ready),
        //   each probe from the same peer still costs a HashMap lookup
        //   + state lookup. Applying the rate-limit FIRST short-circuits
        //   on the timestamp before the tier check.
        //
        // Bypass via varying `nonce` is blocked because the rate-limit
        // key is (peer, namespace), not probe content.
        //
        // Tier-promotion fairness: if we drop a probe due to non-Ready
        // tier, we still record `last_probe_response_at` so the same
        // peer cannot poll us into pathological recheck rates. After a
        // tier transition into *Ready, `last_probe_response_at` for
        // affected (peer, ns) is cleared in `LocalStateChanged` /
        // `ApplyBeaconLocal` so a later probe immediately gets a beacon.
        let now = Instant::now();
        let min_spacing = self.config.beacon_interval / 2;
        let key = (msg.requesting_peer, msg.namespace_id);
        if let Some(last) = self.last_probe_response_at.get(&key) {
            if now.duration_since(*last) < min_spacing {
                return; // within rate-limit window — drop silently
            }
        }

        // Snapshot-then-emit so `publish_beacon` does not borrow
        // `state_per_namespace` across the call (it loads identity from
        // `self.datastore`).
        let snapshot = match self.state_per_namespace.get(&msg.namespace_id) {
            Some(s)
                if matches!(
                    s.tier,
                    ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady
                ) =>
            {
                Some(s.clone())
            }
            _ => None,
        };
        // Stamp BEFORE potential publish: the rate-limit budget is
        // consumed by *this probe* regardless of whether we publish.
        let _ = self.last_probe_response_at.insert(key, now);
        if let Some(snapshot) = snapshot {
            self.publish_beacon(msg.namespace_id, &snapshot);
        }
    }
}

impl ReadinessManager {
    /// Clear rate-limit timestamps for `ns_id` after a tier transition
    /// into a *Ready variant so a probe that arrives shortly after the
    /// promotion gets a fresh beacon instead of being silently dropped
    /// by the still-running rate-limit window.
    fn clear_probe_window_for(&mut self, ns_id: [u8; 32]) {
        self.last_probe_response_at
            .retain(|(_, key_ns), _| *key_ns != ns_id);
    }
}

impl Handler<ApplyBeaconLocal> for ReadinessManager {
    type Result = ();

    fn handle(&mut self, msg: ApplyBeaconLocal, _ctx: &mut Self::Context) {
        // A peer beacon has just been inserted into the cache. Re-evaluate
        // the FSM with the (possibly) updated `peer_summary` and edge-emit
        // if our tier transitions into *Ready.
        let Some(state) = self.state_per_namespace.get(&msg.namespace_id).cloned() else {
            return;
        };
        let peers = self
            .cache
            .peer_summary(msg.namespace_id, self.config.ttl_heartbeat);
        let new_tier = evaluate_readiness(&state, &peers, &self.config, Instant::now());
        if new_tier != state.tier {
            let snapshot = if let Some(s) = self.state_per_namespace.get_mut(&msg.namespace_id) {
                s.tier = new_tier;
                if matches!(
                    new_tier,
                    ReadinessTier::PeerValidatedReady | ReadinessTier::LocallyReady
                ) {
                    Some(s.clone())
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(snapshot) = snapshot {
                self.clear_probe_window_for(msg.namespace_id);
                self.publish_beacon(msg.namespace_id, &snapshot);
            }
        }
    }
}

/// Per-namespace `tokio::sync::Notify` registry that wakes waiters
/// blocked on [`ReadinessCache::await_first_fresh_beacon`] when a
/// beacon arrives.
///
/// Lives on `NodeManager` (alongside `readiness_cache`) so the
/// receiver-side beacon handler in
/// `handlers::network_event::readiness::handle_readiness_beacon` can
/// call `notify.notify(ns)` after `cache.insert(&beacon)` without
/// going through the actor mailbox.
#[derive(Debug, Default)]
pub struct ReadinessCacheNotify {
    waiters: Mutex<HashMap<[u8; 32], Arc<tokio::sync::Notify>>>,
}

impl ReadinessCacheNotify {
    /// Acquire the waiters map, recovering from a poisoned mutex.
    /// See [`ReadinessCache::entries_lock`] for rationale (mirrors
    /// `AckRouter::lock` from PR #2264).
    fn waiters_lock(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<[u8; 32], Arc<tokio::sync::Notify>>> {
        self.waiters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Get-or-create the per-namespace `Notify`. Cloned so the caller
    /// holds it across `.await` points without keeping the registry
    /// lock.
    pub fn waiter_for(&self, ns: [u8; 32]) -> Arc<tokio::sync::Notify> {
        let mut g = self.waiters_lock();
        g.entry(ns)
            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
            .clone()
    }

    pub fn notify(&self, ns: [u8; 32]) {
        let g = self.waiters_lock();
        if let Some(n) = g.get(&ns) {
            n.notify_waiters();
        }
    }
}

impl ReadinessCache {
    /// Block until a fresh-within-`ttl` beacon for `ns` is available
    /// in the cache, or `deadline` elapses.
    ///
    /// Avoids the classic `Notify` race:
    /// `tokio::sync::Notify::notify_waiters()` does NOT store a permit
    /// — it only wakes already-registered waiters. A naive
    /// `pick_sync_partner` (miss) → `waiter.notified().await` ordering
    /// would lose any beacon inserted *between* those two steps,
    /// blocking until the next beacon or `deadline`.
    ///
    /// Fix: register the `Notified` future via `enable()` (tokio
    /// ≥ 1.32) BEFORE the cache check on every iteration. Any
    /// subsequent `notify_waiters()` then wakes us, even if it fires
    /// before we reach the `select!`. The race-test
    /// `await_first_fresh_beacon_resolves_on_late_arrival` exercises
    /// this exact ordering.
    pub async fn await_first_fresh_beacon(
        &self,
        notify: &ReadinessCacheNotify,
        ns: [u8; 32],
        ttl: Duration,
        deadline: Duration,
    ) -> Option<(PublicKey, CacheEntry)> {
        let waiter = notify.waiter_for(ns);
        let timeout_fut = tokio::time::sleep(deadline);
        tokio::pin!(timeout_fut);
        loop {
            // 1. Create + pin a fresh Notified for this iteration.
            let notified = waiter.notified();
            tokio::pin!(notified);
            // 2. Register without polling. From here on, any
            //    `notify_waiters()` is guaranteed to wake us, even if
            //    it fires before we reach the `select!`.
            notified.as_mut().enable();
            // 3. Safe to check the cache — beacons arriving between
            //    `enable()` and the `select!` poll wake the
            //    (already-registered) future.
            if let Some(entry) = self.pick_sync_partner(ns, ttl) {
                return Some(entry);
            }
            tokio::select! {
                _ = notified => { /* loop, re-register, re-check */ }
                _ = &mut timeout_fut => return None,
            }
        }
    }
}
