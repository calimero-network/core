//! Peer-selection helpers for the sync manager.
//!
//! This module owns the "given a context, return peers to sync with"
//! concern, extracted from `manager/mod.rs` as Phase 1 of the
//! `SyncManager` decomposition. The functions here:
//!
//! 1. **Discover** mesh peers for a context's gossipsub topic, with
//!    bounded retry while the mesh forms, and a namespace-topic
//!    fallback when the context-specific mesh hasn't come up yet.
//! 2. **Prioritise** the discovered peer list — anchors first, plain
//!    members after, stable within each partition so the caller's
//!    pre-shuffle randomness is preserved.
//!
//! Tests live alongside this module so the logic can be exercised
//! against [`crate::sync::network::mock::MockSyncNetwork`] +
//! [`crate::sync::state_access_mock::MockSyncStateAccess`] without
//! spinning up a full `SyncManager`.

use std::collections::BTreeSet;
use std::time::Duration;

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use tokio::time;
use tracing::{debug, warn};

use super::network::SyncNetwork;
use super::state_access::SyncStateAccess;

/// Result of `discover_mesh_peers_with_namespace_fallback` reporting
/// which discovery path actually yielded the returned peers. Useful
/// for telemetry and for the manager to decide downstream behaviour
/// (e.g. a namespace-fallback peer is less specific than a context-
/// mesh peer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PeerSource {
    /// Found peers in the context's own gossipsub topic mesh.
    ContextMesh,
    /// Context mesh was empty across all retries; found peers in the
    /// namespace topic mesh as a fallback (namespace meshes are
    /// established earlier during join with a grace period).
    NamespaceFallback,
}

/// Outcome of the discovery loop: the peer list plus diagnostics
/// (which mesh produced them, how many retries elapsed, total
/// elapsed time).
#[derive(Debug)]
pub(crate) struct DiscoveryOutcome {
    pub(crate) peers: Vec<PeerId>,
    pub(crate) source: PeerSource,
    pub(crate) attempts: u32,
    pub(crate) elapsed: Duration,
}

/// Discover mesh peers for `context_id`, retrying the context topic
/// up to `max_retries` rounds with `retry_delay` between attempts.
///
/// If the context-topic mesh stays empty across all retries, fall back
/// to the namespace-topic mesh — namespace meshes are established
/// during join with a grace period, so they're reachable even when
/// the context-specific gossipsub mesh hasn't completed its 5-10
/// heartbeat formation window after subscription. Direct-stream
/// context sync works over any connected P2P peer, so a namespace
/// peer is a sound source.
///
/// `resolve_namespace_topic` is invoked only when the context mesh is
/// empty; it returns `Some(topic_hash)` if the caller can resolve the
/// namespace root for this context, or `None` if not (no group
/// mapping, store error, etc.). The caller is responsible for the
/// context→namespace lookup because that requires the context client
/// which this module deliberately doesn't depend on.
///
/// Returns `Err(_)` if both context-mesh discovery and namespace
/// fallback yield zero peers — the caller bails the sync attempt at
/// that point.
pub(crate) async fn discover_mesh_peers_with_namespace_fallback(
    sync_network: &dyn SyncNetwork,
    context_id: ContextId,
    max_retries: u32,
    retry_delay: Duration,
    resolve_namespace_topic: impl FnOnce() -> Option<TopicHash>,
) -> eyre::Result<DiscoveryOutcome> {
    let discovery_started = std::time::Instant::now();
    let context_topic = TopicHash::from_raw(context_id);

    let mut peers = Vec::new();
    let mut final_attempt = 0u32;
    for attempt in 1..=max_retries {
        final_attempt = attempt;
        peers = sync_network.subscribed_peers(context_topic.clone()).await;
        if !peers.is_empty() {
            break;
        }
        if attempt < max_retries {
            debug!(
                %context_id,
                attempt,
                max_retries,
                "No peers found yet, mesh may still be forming, retrying..."
            );
            time::sleep(retry_delay).await;
        }
    }

    if !peers.is_empty() {
        // The caller logs the success at info with the full field set
        // (peer_count, attempts, elapsed, source); avoid duplicating it
        // here. Keep the warn on the bail path below — the caller bails
        // on Err and would otherwise log nothing.
        return Ok(DiscoveryOutcome {
            peers,
            source: PeerSource::ContextMesh,
            attempts: final_attempt,
            elapsed: discovery_started.elapsed(),
        });
    }

    // Context-topic mesh empty across all retries. Try the namespace
    // topic if the caller can resolve one — the namespace mesh is
    // formed during join with a 2-second grace period, so it's
    // typically reachable while the context mesh is still forming.
    //
    // #2422 Options 3+4 — design intent:
    //
    // The retry loop above is the STEADY-STATE filter: when peers
    // have subscribed to the context topic, the loop returns them
    // directly and we never enter this branch. The branch fires
    // when context-topic gossipsub mesh is empty after the full
    // retry window, which means one of:
    //   (a) No namespace member has materialised this context yet
    //       (e.g. workflows #4 / #5 — every other ns member is
    //       opted-out of auto-follow).
    //   (b) Cold-start race: a peer subscribed locally but the
    //       gossipsub heartbeat hasn't propagated to us yet.
    //
    // For (a): the intersection is genuinely empty and bailing is
    // the correct response — pre-fix we would have dialed unfiltered
    // ns_peers and provoked `inbound stream for unknown context`
    // spam plus a failure_count climb into 256s backoff on every
    // sync tick. Post-fix we bail and let the next tick try again;
    // Option 4's typed NotMaterialized then makes any racy dial
    // benign.
    //
    // For (b): the second `mesh_peers(context_topic)` call below
    // captures any gossipsub progress that happened during the
    // retry loop's backoff window. If a heartbeat propagated since
    // the loop's last attempt, the intersection now contains the
    // newly-visible follower. This is not redundant with the loop —
    // the loop fetched `mesh_peers(context_topic)` only; we now
    // fetch it once more AFTER fetching ns_peers, so a peer that
    // newly subscribed is visible.
    //
    // Context-topic subscription is materialisation-gated — only
    // `node_client.subscribe(&context_id)` adds it, called from
    // `create_context`, `join_context`, and `join_group`'s
    // subscribe arm — so the intersection equals "namespace peer
    // that has a local entry for this context".
    if let Some(ns_topic) = resolve_namespace_topic() {
        let ns_peers = sync_network.subscribed_peers(ns_topic).await;
        if !ns_peers.is_empty() {
            // Two `mesh_peers` calls (ns_topic above, context_topic
            // below) are not snapshot-atomic — gossipsub state can
            // shift between them. The race is benign: a peer that
            // joins the context topic between the two reads gets
            // filtered out (not dialed this tick, picked up on the
            // next), and a peer that leaves between them gets
            // filtered out (correctly — they no longer follow). No
            // correctness hazard, just a one-tick discovery delay.
            let ns_candidate_count = ns_peers.len();
            let ctx_subscribers: BTreeSet<PeerId> = sync_network
                .subscribed_peers(context_topic.clone())
                .await
                .into_iter()
                .collect();
            let filtered: Vec<PeerId> = ns_peers
                .into_iter()
                .filter(|peer| ctx_subscribers.contains(peer))
                .collect();
            if !filtered.is_empty() {
                // Caller's success log carries `source = NamespaceFallback`,
                // which already communicates that the context mesh was
                // empty and the fallback fired — no separate info line
                // needed here.
                return Ok(DiscoveryOutcome {
                    peers: filtered,
                    source: PeerSource::NamespaceFallback,
                    attempts: final_attempt,
                    elapsed: discovery_started.elapsed(),
                });
            }
            debug!(
                %context_id,
                ns_candidates = ns_candidate_count,
                ctx_subscribers = ctx_subscribers.len(),
                "namespace fallback found peers but none subscribe to the context topic; \
                 not dialing — see #2422"
            );
        }
    }

    let elapsed = discovery_started.elapsed();
    warn!(
        %context_id,
        attempts = max_retries,
        ?elapsed,
        "Mesh peer discovery exhausted all retries (context mesh + namespace fallback)"
    );
    // Typed error so `apply_session_result` can recognise "no peer right
    // now" as benign (no failure_count bump, no exponential-backoff warn)
    // rather than a sync failure. Display still contains "No peers to
    // sync with" for log/grep continuity.
    Err(eyre::Error::new(super::manager::NoPeersAvailable {
        context_id,
    }))
}

/// Stable-partition `peers` so peers with an observed trusted-anchor
/// identity come first while preserving the relative order within each
/// partition. Returns the index at which non-anchor peers start (i.e.
/// the count of anchor peers).
///
/// A peer is an anchor if at least one identity recorded in
/// `peer_identities` for that peer appears in `anchors`. An empty
/// `anchors` set returns 0 immediately — no point sorting if every
/// peer is going to be non-anchor.
///
/// The anchor predicate is materialised into a `Vec<bool>` keyed by
/// the peer's original index before sorting. This avoids reacquiring
/// the `peer_identities` lock O(n log n) times during `sort_by_key`'s
/// comparisons, and prevents a concurrent cache mutation from causing
/// the post-sort anchor count to disagree with the actual partition
/// boundary — both `sort_by_key` and the count read from the same
/// snapshot.
///
/// Free function (not a method) so it can be unit-tested against
/// synthetic inputs without spinning up a sync manager.
pub(crate) fn partition_peers_anchor_first(
    peers: &mut [PeerId],
    state_access: &dyn SyncStateAccess,
    anchors: &BTreeSet<PublicKey>,
) -> usize {
    if anchors.is_empty() {
        return 0;
    }
    let anchor_flags: Vec<bool> = peers
        .iter()
        .map(|peer| {
            state_access
                .peer_identities(peer)
                .map(|ids| ids.iter().any(|id| anchors.contains(id)))
                .unwrap_or(false)
        })
        .collect();
    // sort_by_key over a pre-indexed flag table — stable, so the
    // caller's random shuffle order is preserved within each partition.
    let mut indices: Vec<usize> = (0..peers.len()).collect();
    indices.sort_by_key(|&i| !anchor_flags[i]);
    let anchor_count = anchor_flags.iter().filter(|&&f| f).count();
    let reordered: Vec<PeerId> = indices.iter().map(|&i| peers[i]).collect();
    peers.copy_from_slice(&reordered);
    anchor_count
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::sync::network::mock::MockSyncNetwork;
    use crate::sync::state_access_mock::MockSyncStateAccess;

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    fn dummy_peer(n: u8) -> PeerId {
        let seed = [n; 32];
        let kp = libp2p::identity::Keypair::ed25519_from_bytes(seed).expect("valid seed");
        PeerId::from_public_key(&kp.public())
    }

    fn dummy_pk(n: u8) -> PublicKey {
        PublicKey::from([n; 32])
    }

    // ---- discover_mesh_peers_with_namespace_fallback ----

    /// Context-mesh has peers on the first attempt → no retry, no
    /// fallback. Source is `ContextMesh`.
    #[tokio::test(start_paused = true)]
    async fn discovery_returns_context_mesh_on_first_attempt() {
        let mock = MockSyncNetwork::default();
        let context_id = ctx(0xAA);
        let context_topic = TopicHash::from_raw(context_id);
        let peer = dummy_peer(1);
        mock.push_subscribed_peers_for(context_topic, vec![peer]);

        let outcome = discover_mesh_peers_with_namespace_fallback(
            &mock,
            context_id,
            3,
            Duration::from_millis(50),
            || None,
        )
        .await
        .expect("ok");

        assert_eq!(outcome.peers, vec![peer]);
        assert_eq!(outcome.source, PeerSource::ContextMesh);
        assert_eq!(outcome.attempts, 1, "must succeed on the first attempt");
    }

    /// Context-mesh empty across all retries AND no namespace fallback
    /// available → Err. The discovery loop hits its retry budget and
    /// bails. Matches the production contract.
    #[tokio::test(start_paused = true)]
    async fn discovery_errs_when_context_empty_and_no_namespace_fallback() {
        let mock = MockSyncNetwork::default();
        // Nothing seeded → mock returns empty for every topic.

        let result = discover_mesh_peers_with_namespace_fallback(
            &mock,
            ctx(0xAA),
            3,
            Duration::from_millis(50),
            || None,
        )
        .await;

        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No peers to sync with"),
            "unexpected err: {err}"
        );
    }

    /// Both meshes empty (context topic exhausted retries AND the
    /// namespace fallback also returned no peers) → Err. Distinct
    /// from `discovery_errs_when_context_empty_and_no_namespace_fallback`,
    /// which exercises the no-resolver branch. Neither topic is seeded,
    /// so both fall through to the (empty) shared queue.
    #[tokio::test(start_paused = true)]
    async fn discovery_errs_when_both_meshes_empty() {
        let mock = MockSyncNetwork::default();
        let ns_topic = TopicHash::from_raw("ns/fake");

        let result = discover_mesh_peers_with_namespace_fallback(
            &mock,
            ctx(0xAA),
            2,
            Duration::from_millis(10),
            || Some(ns_topic),
        )
        .await;

        assert!(
            result.is_err(),
            "both context mesh and namespace mesh empty → Err"
        );
    }

    /// The namespace-fallback arm intersects the namespace peer set
    /// with the context-topic subscribers, so a namespace member who
    /// DIDN'T subscribe to the context (auto-follow opted out,
    /// JoinContext in flight, etc.) is filtered out and not dialed.
    ///
    /// Per-topic queues let each topic be scripted independently:
    ///   - context topic: empty for both retry attempts, then the
    ///     subscriber subset for the post-fallback intersection lookup
    ///     (three reads of the same topic across phases → a three-entry
    ///     sticky-last sequence);
    ///   - namespace topic: the full namespace peer set.
    /// The result should be namespace_peers ∩ context_subscribers.
    #[tokio::test(start_paused = true)]
    async fn discovery_filters_namespace_peers_by_context_subscription() {
        let mock = MockSyncNetwork::default();
        let follower = dummy_peer(1);
        let opted_out = dummy_peer(2);

        let context_id = ctx(0xAA);
        let context_topic = TopicHash::from_raw(context_id);
        let ns_topic = TopicHash::from_raw("ns/fake");

        // Context topic: two empty reads exhaust the retry budget, then
        // the intersection lookup sees only `follower` subscribed.
        mock.push_subscribed_peers_for(context_topic.clone(), vec![])
            .push_subscribed_peers_for(context_topic.clone(), vec![])
            .push_subscribed_peers_for(context_topic, vec![follower])
            // Namespace topic: both candidates are namespace members.
            .push_subscribed_peers_for(ns_topic.clone(), vec![follower, opted_out]);

        let outcome = discover_mesh_peers_with_namespace_fallback(
            &mock,
            context_id,
            2,
            Duration::from_millis(10),
            || Some(ns_topic),
        )
        .await
        .expect("intersection should return follower");

        assert_eq!(outcome.peers, vec![follower]);
        assert_eq!(outcome.source, PeerSource::NamespaceFallback);
        assert!(
            !outcome.peers.contains(&opted_out),
            "opted-out namespace member must be filtered out"
        );
    }

    /// Companion to `discovery_filters_namespace_peers_by_context_subscription`:
    /// if EVERY namespace member has opted out of the context (the
    /// intersection is empty), discovery bails with the same "no peers"
    /// error rather than falling back to the unfiltered namespace list.
    #[tokio::test(start_paused = true)]
    async fn discovery_errs_when_no_namespace_peer_subscribes_to_context() {
        let mock = MockSyncNetwork::default();
        let opted_out_a = dummy_peer(1);
        let opted_out_b = dummy_peer(2);

        let context_id = ctx(0xAA);
        let context_topic = TopicHash::from_raw(context_id);
        let ns_topic = TopicHash::from_raw("ns/fake");

        // Context topic empty throughout (retries AND the intersection
        // lookup → no context subscribers); namespace topic has two
        // peers, both of which fail the intersection.
        mock.push_subscribed_peers_for(context_topic, vec![])
            .push_subscribed_peers_for(ns_topic.clone(), vec![opted_out_a, opted_out_b]);

        let result = discover_mesh_peers_with_namespace_fallback(
            &mock,
            context_id,
            2,
            Duration::from_millis(10),
            || Some(ns_topic),
        )
        .await;

        assert!(
            result.is_err(),
            "all namespace peers opted out → no candidates → Err"
        );
    }

    // ---- partition_peers_anchor_first ----
    //
    // Tests for the partition function previously lived in
    // `manager/tests.rs::partition_*`. Move alongside the function
    // they cover. The shape of the tests is unchanged — they exercise
    // the same invariant (anchor peers stable-first, non-anchors
    // stable-second).

    fn node_state_with_peer_identities(
        entries: impl IntoIterator<Item = (PeerId, BTreeSet<PublicKey>)>,
    ) -> crate::state::NodeState {
        let node_state = crate::state::NodeState::new(false, crate::run::NodeMode::Standard);
        for (peer, ids) in entries {
            let _replaced = node_state.peer_identities.insert(peer, ids);
        }
        node_state
    }

    #[test]
    fn partition_empty_anchors_set_returns_zero() {
        let mut peers = vec![dummy_peer(1), dummy_peer(2), dummy_peer(3)];
        let node_state = node_state_with_peer_identities([]);
        let anchors: BTreeSet<PublicKey> = BTreeSet::new();

        let count = partition_peers_anchor_first(&mut peers, &node_state, &anchors);
        assert_eq!(count, 0);
    }

    #[test]
    fn partition_empty_cache_no_anchors_found() {
        let mut peers = vec![dummy_peer(1), dummy_peer(2)];
        let original = peers.clone();
        let node_state = node_state_with_peer_identities([]);
        let anchors: BTreeSet<PublicKey> = [dummy_pk(0xAA)].into_iter().collect();

        let count = partition_peers_anchor_first(&mut peers, &node_state, &anchors);
        assert_eq!(count, 0);
        assert_eq!(peers, original);
    }

    #[test]
    fn partition_all_peers_are_anchors() {
        let peer1 = dummy_peer(1);
        let peer2 = dummy_peer(2);
        let pk_admin = dummy_pk(0xAA);
        let node_state = node_state_with_peer_identities([
            (peer1, [pk_admin].into_iter().collect()),
            (peer2, [pk_admin].into_iter().collect()),
        ]);
        let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

        let mut peers = vec![peer1, peer2];
        let count = partition_peers_anchor_first(&mut peers, &node_state, &anchors);
        assert_eq!(count, 2);
        assert_eq!(peers, vec![peer1, peer2]);
    }

    #[test]
    fn partition_mixed_anchor_and_non_anchor_preserves_relative_order() {
        let anchor_a = dummy_peer(1);
        let anchor_b = dummy_peer(2);
        let plain_a = dummy_peer(3);
        let plain_b = dummy_peer(4);
        let pk_admin = dummy_pk(0xAA);

        let node_state = node_state_with_peer_identities([
            (anchor_a, [pk_admin].into_iter().collect()),
            (anchor_b, [pk_admin].into_iter().collect()),
        ]);

        let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

        let mut peers = vec![plain_a, anchor_a, plain_b, anchor_b];
        let count = partition_peers_anchor_first(&mut peers, &node_state, &anchors);
        assert_eq!(count, 2);
        assert_eq!(peers, vec![anchor_a, anchor_b, plain_a, plain_b]);
    }

    #[test]
    fn partition_peer_with_one_anchor_identity_among_many_qualifies() {
        let peer = dummy_peer(1);
        let pk_admin = dummy_pk(0xAA);
        let pk_other_context = dummy_pk(0xBB);

        let node_state = node_state_with_peer_identities([(
            peer,
            [pk_admin, pk_other_context].into_iter().collect(),
        )]);

        let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

        let mut peers = vec![peer];
        let count = partition_peers_anchor_first(&mut peers, &node_state, &anchors);
        assert_eq!(count, 1);
    }

    #[test]
    fn partition_peer_with_only_non_anchor_identities_does_not_qualify() {
        let peer = dummy_peer(1);
        let pk_member = dummy_pk(0xCC);
        let pk_admin = dummy_pk(0xAA);

        let node_state =
            node_state_with_peer_identities([(peer, [pk_member].into_iter().collect())]);

        let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

        let mut peers = vec![peer];
        let count = partition_peers_anchor_first(&mut peers, &node_state, &anchors);
        assert_eq!(count, 0);
    }

    /// Partition works against any `SyncStateAccess` impl, not just
    /// `NodeState`. Exercising via `MockSyncStateAccess` is the
    /// test-surface the trait promised — same behaviour, no
    /// `NodeState` needed.
    #[test]
    fn partition_works_against_mock_sync_state_access() {
        use crate::sync::state_access_mock::{MockSyncStateAccess, SyncStateAccessCall};

        let anchor_peer = dummy_peer(1);
        let plain_peer = dummy_peer(2);
        let pk_admin = dummy_pk(0xAA);

        let mock = MockSyncStateAccess::default();
        mock.insert_peer_identities(anchor_peer, [pk_admin].into_iter().collect());

        let anchors: BTreeSet<PublicKey> = [pk_admin].into_iter().collect();

        let mut peers = vec![plain_peer, anchor_peer];
        let count = partition_peers_anchor_first(&mut peers, &mock, &anchors);
        assert_eq!(count, 1);
        assert_eq!(peers, vec![anchor_peer, plain_peer]);

        let calls = mock.calls();
        assert_eq!(
            calls,
            vec![
                SyncStateAccessCall::PeerIdentities(plain_peer),
                SyncStateAccessCall::PeerIdentities(anchor_peer),
            ]
        );
    }
}
