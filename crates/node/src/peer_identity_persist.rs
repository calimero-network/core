//! Datastore glue for [`PeerIdentityCache`]: snapshot it to a `Generic`
//! key on a periodic tick, and hydrate it (plus the in-memory
//! `peer_identities` reverse view) on startup.
//!
//! Mirrors the network crate's `PeerAddrCache` persistence: a single
//! best-effort blob under one key, written on a tick and read on
//! startup. The whole point is that the authenticated member→peer
//! signal survives a restart so anchor-preferred sync selection works on
//! a cold cache instead of falling back to random topic subscribers.

use std::collections::BTreeMap;

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::op_events::{self, OpEvent};
use calimero_network_primitives::client::NetworkClient;
use calimero_store::key::Generic as GenericKey;
use calimero_store::slice::Slice;
use calimero_store::types::GenericData;
use calimero_store::Store;
use libp2p::PeerId;
use tracing::{debug, info, warn};

use crate::peer_identity_cache::{PeerIdentityCache, PeerScoreTier, PEER_IDENTITY_TTL_SECS};
use crate::state::{now_unix_secs, NodeState};

/// How often the snapshot tick writes the cache to disk. Matches the
/// metrics tick's cadence — frequent enough that a crash loses little,
/// rare enough that the write is negligible against node activity.
const SNAPSHOT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Datastore key for the single peer-identity-cache blob. 16-byte scope
/// distinct from `PeerAddrCache`'s `calimero-peercch`; fragment is unused
/// (one blob, like the address cache).
fn store_key() -> GenericKey {
    GenericKey::new(*b"calimero-idpeers", [0u8; 32])
}

/// Serialize the cache's still-fresh entries and write them to the store.
/// Best-effort: a serialize/put failure is logged at debug and dropped —
/// the cache is a routing hint, never load-bearing. Skips the write
/// entirely when nothing is cached, to avoid churning an empty blob while
/// the node is idle.
pub(crate) fn persist(state: &NodeState, store: &Store) {
    let blob = state
        .lock_peer_identity_cache()
        .to_persisted_all(now_unix_secs(), PEER_IDENTITY_TTL_SECS);
    if blob.groups.is_empty() {
        return;
    }
    let bytes = match serde_json::to_vec(&blob) {
        Ok(bytes) => bytes,
        Err(err) => {
            debug!(?err, "failed to serialize peer-identity cache");
            return;
        }
    };
    let data = GenericData::from(Slice::from(bytes));
    let mut handle = store.handle();
    if let Err(err) = handle.put(&store_key(), &data) {
        debug!(?err, "failed to persist peer-identity cache to store");
    }
}

/// Load the cache from the store on startup and hydrate both it and the
/// in-memory `peer_identities` reverse view, so anchor-preferred
/// selection has a membership signal immediately rather than after live
/// traffic refills it. Best-effort: a missing or corrupt blob leaves the
/// caches empty (the pre-persistence behaviour) rather than failing.
pub(crate) fn hydrate(state: &NodeState, store: &Store) {
    let now = now_unix_secs();
    let blob = match store.handle().get(&store_key()) {
        Ok(Some(data)) => match serde_json::from_slice(data.as_ref()) {
            Ok(blob) => blob,
            Err(err) => {
                // warn (not debug): a corrupt/partial blob silently loses
                // every cold-start hint, which an operator should be able
                // to see. Still best-effort — carry on with an empty cache
                // that refills from live traffic.
                warn!(?err, "ignoring corrupt peer-identity cache blob in store");
                return;
            }
        },
        Ok(None) => return, // nothing cached yet
        Err(err) => {
            debug!(?err, "failed to read peer-identity cache from store");
            return;
        }
    };

    let cache = PeerIdentityCache::load_all_from_persisted(blob, now, PEER_IDENTITY_TTL_SECS);
    let pairs = cache.all_peer_identity_pairs();
    let pair_count = pairs.len();

    // Publish the durable cache FIRST, then seed the reverse view. Order
    // matters for the (startup-only) case where an `observe_peer_identity`
    // could run concurrently: once the cache is the new one, a concurrent
    // observe records into it and isn't lost. We deliberately do NOT hold
    // the cache lock across the `peer_identities` (DashMap) seeding — that
    // would invert the lock order `observe_peer_identity` uses (DashMap
    // then cache) and risk a deadlock. In practice hydrate runs at startup
    // before the event loop dispatches ops, so no observer races it.
    *state.lock_peer_identity_cache() = cache;
    for (peer, identity) in pairs {
        let _ = state
            .peer_identities
            .entry(peer)
            .or_default()
            .insert(identity);
    }

    if pair_count > 0 {
        info!(
            pair_count,
            "hydrated peer-identity cache from store for cold-start member selection"
        );
    }
}

/// Spawn the periodic snapshot task. Holds a strong `NodeState`/`Store`
/// reference for the runtime's lifetime — a missed snapshot during
/// shutdown is harmless, so no shutdown plumbing (same rationale as the
/// metrics tick). The returned handle is stored as `_…` by the caller;
/// dropping it does not cancel the task (tokio detaches it), which runs
/// until the runtime is dropped. A future graceful shutdown could
/// `abort()` it.
pub(crate) fn spawn_snapshot_tick(
    state: NodeState,
    store: Store,
    network: NetworkClient,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(SNAPSHOT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Consume the immediate first fire so the first snapshot lands at
        // startup + INTERVAL, not instantly (nothing has been observed
        // yet at startup, and `persist` would just no-op on the empty
        // cache anyway).
        let _ = interval.tick().await;
        loop {
            let _ = interval.tick().await;
            persist(&state, &store);
            reconcile_peer_scores(&state, &network);
        }
    })
}

/// Diff the desired per-peer gossipsub score tier (derived from the
/// cache) against the last-pushed tracker (#2513). Returns the peers
/// whose tier changed (with the new tier) and the peers that dropped out
/// of the cache entirely (to be cleared to 0). Pure and transition-
/// guarded: an unchanged peer produces no update. The caller does the
/// network pushes and tracker mutation.
fn compute_score_updates(
    cache: &PeerIdentityCache,
    tracker: &BTreeMap<PeerId, PeerScoreTier>,
    now_secs: u64,
    ttl_secs: u64,
) -> (Vec<(PeerId, PeerScoreTier)>, Vec<PeerId>) {
    // Desired tier per peer = the strongest tier across every group/member
    // it currently hosts.
    let mut desired: BTreeMap<PeerId, PeerScoreTier> = BTreeMap::new();
    for group in cache.groups() {
        for member in cache.members_for_group(group, now_secs, ttl_secs) {
            let tier = PeerScoreTier::from_role(&member.role);
            for peer in member.peers {
                let entry = desired.entry(peer).or_insert(tier);
                *entry = (*entry).max(tier);
            }
        }
    }
    let pushes = desired
        .iter()
        .filter(|(peer, tier)| tracker.get(peer) != Some(tier))
        .map(|(peer, tier)| (*peer, *tier))
        .collect();
    let clears = tracker
        .keys()
        .filter(|peer| !desired.contains_key(peer))
        .copied()
        .collect();
    (pushes, clears)
}

/// Reconcile gossipsub peer scores against current cached membership and
/// push the deltas to the network layer (#2513). Runs on the snapshot
/// tick: new/upgraded members get a positive score, members that left
/// the cache (removed via `MemberRemoved`, or aged out) get cleared to 0.
fn reconcile_peer_scores(state: &NodeState, network: &NetworkClient) {
    let now = now_unix_secs();
    let (pushes, clears) = {
        let cache = state.lock_peer_identity_cache();
        let tracker = state.lock_peer_scores();
        compute_score_updates(&cache, &tracker, now, PEER_IDENTITY_TTL_SECS)
    };
    if pushes.is_empty() && clears.is_empty() {
        return;
    }
    let mut tracker = state.lock_peer_scores();
    for (peer, tier) in pushes {
        network.set_peer_score(peer, tier.score());
        let _ = tracker.insert(peer, tier);
    }
    for peer in clears {
        network.set_peer_score(peer, 0.0);
        let _ = tracker.remove(&peer);
    }
}

/// Apply one op-apply event to the cache. Currently only `MemberRemoved`
/// is actionable: it drops the removed member from its group's bucket so
/// a removed member stops being preferred for sync (and stops being
/// re-persisted) promptly, rather than after the 24h TTL. Other events
/// are ignored. Kept separate from the async loop so it's unit-testable.
fn apply_invalidation_event(state: &NodeState, event: &OpEvent) {
    if let OpEvent::MemberRemoved { group_id, member } = event {
        // Only the durable cache's per-group membership view is dropped.
        // The in-memory `peer_identities` reverse view is deliberately
        // left intact: the peer still *controls* that identity (removal
        // changes group membership, not key ownership), and that view is
        // only ever intersected with the authoritative `trusted_anchors`
        // set at selection time — which no longer lists the removed member
        // — so a stale reverse entry can't make the peer an anchor. The
        // `member_removed_event_drops_cached_member` test pins this
        // (asserts the reverse view is untouched) so a future "cleanup"
        // doesn't silently change it.
        state
            .lock_peer_identity_cache()
            .remove_member(&ContextGroupId::from(*group_id), member);
        debug!(
            group_id = %hex::encode(group_id),
            %member,
            "dropped removed member from peer-identity cache"
        );
    }
}

/// Spawn the cache-invalidation task: subscribe to governance op-apply
/// events and drop removed members from the cache. The first node-side
/// `op_events` subscriber. A dropped (lagged) event is harmless — the
/// missed member ages out via TTL and is re-derived from the DAG on
/// restart — so the loop just logs and continues. Holds a strong
/// `NodeState` for the runtime's lifetime, like the snapshot tick.
///
/// `op_events::subscribe()` is called **synchronously here, before
/// spawning**, so the receiver starts buffering immediately rather than
/// at some later point when the task first gets scheduled — minimizing
/// the startup window in which a `MemberRemoved` could be missed.
///
/// The returned handle may be dropped (the caller stores it as `_…`); the
/// task then runs detached until the broadcast channel closes
/// (`RecvError::Closed`), i.e. for the process lifetime. There is no
/// graceful-shutdown path because a missed late event is harmless (TTL
/// covers it); a caller that wanted one could `abort()` the handle.
pub(crate) fn spawn_invalidation_task(state: NodeState) -> tokio::task::JoinHandle<()> {
    let mut rx = op_events::subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => apply_invalidation_event(&state, &event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    warn!(
                        skipped,
                        "peer-identity invalidation subscriber lagged; missed MemberRemoved \
                         events age out via TTL and are re-derived on restart"
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PublicKey;
    use calimero_store::db::InMemoryDB;
    use libp2p::PeerId;

    use super::*;
    use crate::peer_identity_cache::ObservedMembership;
    use crate::run::NodeMode;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    #[test]
    fn compute_score_updates_diffs_desired_against_tracker() {
        let mut cache = PeerIdentityCache::default();
        let group = ContextGroupId::from([1u8; 32]);
        let admin_peer = PeerId::random();
        let member_peer = PeerId::random();
        cache.record(
            group,
            PublicKey::from([1u8; 32]),
            admin_peer,
            GroupMemberRole::Admin,
            100,
        );
        cache.record(
            group,
            PublicKey::from([2u8; 32]),
            member_peer,
            GroupMemberRole::Member,
            100,
        );

        // Empty tracker → both peers are fresh pushes at their tiers.
        let empty = BTreeMap::new();
        let (pushes, clears) = compute_score_updates(&cache, &empty, 100, 1000);
        let pushed: BTreeMap<_, _> = pushes.into_iter().collect();
        assert_eq!(pushed.get(&admin_peer), Some(&PeerScoreTier::Anchor));
        assert_eq!(pushed.get(&member_peer), Some(&PeerScoreTier::Member));
        assert!(clears.is_empty());

        // Tracker already matches → no pushes (transition guard).
        let matched = BTreeMap::from([
            (admin_peer, PeerScoreTier::Anchor),
            (member_peer, PeerScoreTier::Member),
        ]);
        let (pushes, clears) = compute_score_updates(&cache, &matched, 100, 1000);
        assert!(pushes.is_empty(), "unchanged tiers produce no push");
        assert!(clears.is_empty());

        // A peer the tracker scored but the cache no longer holds → clear.
        let stranger = PeerId::random();
        let mut stale = matched.clone();
        let _ = stale.insert(stranger, PeerScoreTier::Member);
        let (pushes, clears) = compute_score_updates(&cache, &stale, 100, 1000);
        assert!(pushes.is_empty());
        assert_eq!(clears, vec![stranger], "dropped peer is cleared");
    }

    #[test]
    fn persist_then_hydrate_round_trips_through_store() {
        let store = store();
        let group = ContextGroupId::from([7u8; 32]);
        let identity = PublicKey::from([9u8; 32]);
        let peer = PeerId::random();

        let state = NodeState::new(false, NodeMode::Standard);
        state.observe_peer_identity(
            peer,
            identity,
            Some(ObservedMembership {
                group_id: group,
                role: GroupMemberRole::Admin,
            }),
        );
        persist(&state, &store);

        // A fresh node starts with empty caches, then hydrates from disk.
        let restored = NodeState::new(false, NodeMode::Standard);
        assert!(restored.peer_identities.is_empty(), "starts empty");
        hydrate(&restored, &store);

        // The in-memory reverse view (anchor-filter hot path) is seeded.
        assert!(
            restored
                .peer_identities
                .get(&peer)
                .is_some_and(|ids| ids.contains(&identity)),
            "reverse view hydrated"
        );
        // The durable cache is restored with group + role intact.
        let members = restored.lock_peer_identity_cache().members_for_group(
            &group,
            now_unix_secs(),
            PEER_IDENTITY_TTL_SECS,
        );
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].identity, identity);
        assert_eq!(members[0].role, GroupMemberRole::Admin);
        assert_eq!(members[0].peers, vec![peer]);
    }

    // Exercises the handler directly (`apply_invalidation_event`) rather
    // than via `spawn_invalidation_task` + `op_events::notify` — the
    // `op_events` channel is a process-wide singleton shared across
    // parallel tests, so driving the sync handler directly keeps this
    // test isolated. Prefer this pattern for invalidation-logic tests.
    #[test]
    fn member_removed_event_drops_cached_member() {
        let state = NodeState::new(false, NodeMode::Standard);
        let group = ContextGroupId::from([7u8; 32]);
        let member = PublicKey::from([9u8; 32]);
        let peer = PeerId::random();
        state.observe_peer_identity(
            peer,
            member,
            Some(ObservedMembership {
                group_id: group,
                role: GroupMemberRole::Admin,
            }),
        );
        let cached = |s: &NodeState| {
            !s.lock_peer_identity_cache()
                .members_for_group(&group, now_unix_secs(), PEER_IDENTITY_TTL_SECS)
                .is_empty()
        };
        assert!(cached(&state), "seeded");

        apply_invalidation_event(
            &state,
            &OpEvent::MemberRemoved {
                group_id: [7u8; 32],
                member,
            },
        );
        assert!(!cached(&state), "MemberRemoved dropped the cached member");

        // Intentional: the in-memory reverse view is NOT cleared — the peer
        // still controls the identity, and anchor status is re-derived from
        // trusted_anchors at selection time (see apply_invalidation_event).
        // Pinned here so a future "cleanup" doesn't silently change it.
        assert!(
            state
                .peer_identities
                .get(&peer)
                .is_some_and(|ids| ids.contains(&member)),
            "reverse view deliberately retained after MemberRemoved"
        );
    }

    #[test]
    fn hydrate_with_no_blob_is_a_noop() {
        let store = store();
        let state = NodeState::new(false, NodeMode::Standard);
        hydrate(&state, &store);
        assert!(state.peer_identities.is_empty());
        assert_eq!(state.lock_peer_identity_cache().groups().count(), 0);
    }

    #[test]
    fn observation_without_membership_does_not_persist() {
        let store = store();
        let state = NodeState::new(false, NodeMode::Standard);
        // Namespace-path style: in-memory only, no durable record.
        state.observe_peer_identity(PeerId::random(), PublicKey::from([1u8; 32]), None);
        persist(&state, &store);

        // Nothing was written (empty cache → skipped), so a fresh node
        // hydrates to empty.
        let restored = NodeState::new(false, NodeMode::Standard);
        hydrate(&restored, &store);
        assert_eq!(restored.lock_peer_identity_cache().groups().count(), 0);
        assert!(restored.peer_identities.is_empty());
    }
}
