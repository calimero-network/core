use actix::{AsyncContext, WrapFuture};
use calimero_primitives::context::ContextId;
use tracing::{debug, info, warn};

use crate::NodeManager;

pub(super) fn handle_subscribed(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    peer_id: libp2p::PeerId,
    topic: libp2p::gossipsub::TopicHash,
) {
    // Track every observed subscription so Phase-1 governance readiness
    // (`assert_transport_ready` via `NodeClient::known_subscribers`) can
    // cap the required mesh quorum by the population size. The
    // bookkeeping is topic-agnostic — non-governance topics in the map
    // are harmless because the readiness gate only queries `ns/<id>`
    // and `group/<id>` topics.
    manager
        .clients
        .node
        .record_peer_subscribed(peer_id, topic.clone());

    let topic_str = topic.as_str();

    // Check for group topic: "group/<hex32>"
    if let Some(hex) = topic_str.strip_prefix("group/") {
        let mut bytes = [0u8; 32];
        if hex::decode_to_slice(hex, &mut bytes).is_ok() {
            info!(%peer_id, group_id=%hex, "Peer subscribed to group topic, triggering sync");
            let context_client = manager.clients.context.clone();
            let _ignored = ctx.spawn(
                async move {
                    use calimero_context_client::group::{
                        BroadcastGroupLocalStateRequest, SyncGroupRequest,
                    };
                    use calimero_context_config::types::ContextGroupId;

                    let group_id = ContextGroupId::from(bytes);
                    if let Err(err) = context_client
                        .sync_group(SyncGroupRequest {
                            group_id,
                            requester: None,
                        })
                        .await
                    {
                        warn!(?err, "Failed to auto-sync group after peer subscription");
                    }
                    if let Err(err) = context_client
                        .broadcast_group_local_state(BroadcastGroupLocalStateRequest { group_id })
                        .await
                    {
                        warn!(
                            ?err,
                            "Failed to re-broadcast group local state after peer subscription"
                        );
                    }
                }
                .into_actor(manager),
            );
        }
        return;
    }

    // #2367 — namespace governance topic. A peer just subscribed to
    // `ns/<hex>`; emit an out-of-cycle readiness beacon so the new
    // subscriber sees our namespace DAG head within ~1s instead of
    // waiting up to a full ~5s periodic interval. The
    // `EmitOutOfCycleBeacon` handler no-ops unless we are *Ready in
    // this namespace and rate-limits per (peer, namespace), so this is
    // safe even when the subscribing peer is in a namespace we don't
    // belong to.
    if let Some(hex) = topic_str.strip_prefix("ns/") {
        let mut bytes = [0u8; 32];
        if hex::decode_to_slice(hex, &mut bytes).is_err() {
            debug!(
                %peer_id,
                topic = %topic_str,
                "ns/ topic with malformed namespace id; ignoring subscription"
            );
            return;
        }
        match &manager.readiness_addr {
            Some(addr) => {
                info!(
                    %peer_id,
                    namespace_id = %hex,
                    "Peer subscribed to namespace topic, emitting out-of-cycle beacon"
                );
                addr.do_send(crate::readiness::EmitOutOfCycleBeacon {
                    namespace_id: bytes,
                    requesting_peer: peer_id,
                });
            }
            None => {
                // Readiness actor not yet mounted (early-startup race).
                // The ~5s periodic beacon still covers this subscriber;
                // only the ~1s cold-start speedup is lost.
                debug!(
                    %peer_id,
                    namespace_id = %hex,
                    "ns/ subscription observed before readiness actor mounted; \
                     out-of-cycle beacon skipped"
                );
            }
        }

        // Pull governance state and any key this node is owed, naming the peer
        // that just subscribed as the responder.
        //
        // Both recoveries are normally driven by a peer's `ReadinessBeacon`:
        // the beacon advertises a DAG head, and an unknown head triggers
        // `sync_namespace_from_peer`. That trigger is unreachable for the node
        // that needs it most. `verify_readiness_beacon` requires the signer to
        // be a KNOWN member, and knowing members requires the very governance
        // state a member that joined without ever reaching a mesh peer never
        // received — so it drops every beacon and nothing it hears can move it
        // forward. The key pull has the same problem from the other side: its
        // only other trigger is *receiving* a gossiped Group op, which needs a
        // populated mesh, which a stranded joiner does not have.
        //
        // A peer subscribing to `ns/<id>` breaks that circle without touching
        // either guard. It is our own gossipsub reporting a reachable peer, not
        // a peer-supplied claim, so nothing is being trusted that wasn't before
        // — this only adds a second, un-forgeable trigger for the same pulls,
        // issued at the first moment they can succeed. Relaxing the beacon guard
        // instead would widen what a stranger can assert about membership.
        // The `group/` branch above already drives active recovery on
        // subscription rather than only announcing; this brings `ns/` in line.
        //
        // Gated on the stranded condition itself — this node is a member of a
        // group in the namespace but holds no key for it — rather than firing on
        // every subscription. Two reasons. It bounds the work: a busy namespace
        // raises one of these events per peer, and the beacon path it parallels
        // is debounced where this is not, so an ungated pull would let peer churn
        // drive repeated whole-namespace backfills. And it is self-limiting: the
        // predicate goes false the moment the recovery lands, so a healthy node
        // never pulls from here at all and a recovering one stops on its own.
        // A member that holds its key but is merely behind is left to the beacon
        // path, which it can verify precisely because it is not stranded.
        //
        // Detached via `ctx.spawn` (not `tokio::spawn`) because the sync-manager
        // futures hold non-`Send` types and must stay on this arbiter.
        let store = manager.clients.context.datastore_handle().into_inner();
        match calimero_governance_store::namespace_groups_member_but_keyless(&store, bytes.into()) {
            Ok(groups) if groups.is_empty() => return,
            Ok(_) => {}
            Err(err) => {
                // Unknown state, not "nothing to do". Fall through and let the
                // pulls decide for themselves — they are no-ops when nothing is
                // owed, so a store fault costs a redundant round-trip rather
                // than leaving a genuinely stranded node with no trigger.
                debug!(
                    %peer_id,
                    namespace_id = %hex,
                    %err,
                    "could not evaluate keyless-member state; attempting recovery anyway"
                );
            }
        }

        let sync_manager = manager.managers.sync.clone();
        let _ignored = ctx.spawn(
            async move {
                let ops = sync_manager
                    .sync_namespace_from_peer(bytes, Some(peer_id))
                    .await;
                // Logged unconditionally, including `ops = 0`. Gating this on
                // `ops > 0` would make "the recovery ran and pulled nothing"
                // indistinguishable in the logs from "the recovery never ran" —
                // and a keyless joiner can converge on the key pull below
                // having needed no governance ops at all, so the quiet case is
                // a real outcome, not a non-event. Anyone reading a failure
                // needs to know this path executed.
                debug!(
                    %peer_id,
                    namespace_id = %hex::encode(bytes),
                    ops,
                    "stranded-member recovery: pulled governance from a newly-subscribed namespace peer"
                );
                sync_manager
                    .recover_missing_group_keys(bytes, Some(peer_id))
                    .await;
                debug!(
                    %peer_id,
                    namespace_id = %hex::encode(bytes),
                    "stranded-member recovery: key pull complete"
                );
            }
            .into_actor(manager),
        );

        return;
    }

    let Ok(context_id): Result<ContextId, _> = topic_str.parse() else {
        return;
    };

    match manager.clients.context.has_context(&context_id) {
        Ok(true) => {}
        Ok(false) => {
            debug!(
                %context_id,
                %peer_id,
                "Observed subscription to unknown context, ignoring.."
            );
            return;
        }
        Err(err) => {
            // A store error is unknown state, not "no such context". Surface it
            // and bail rather than silently treating the context as absent.
            warn!(
                %context_id,
                %peer_id,
                %err,
                "has_context lookup failed while handling subscription; ignoring"
            );
            return;
        }
    }

    info!(
        %context_id,
        %peer_id,
        "Peer subscribed to context, triggering sync"
    );

    // Trigger an immediate sync with the peer that just joined this
    // context's mesh, instead of waiting up to a full periodic interval
    // for the next tick to notice it. Mirrors the `group/` branch above.
    // This is the per-context analogue of the post-restart recovery: the
    // moment a co-member appears on the context topic, pull from them so
    // a freshly (re)connected peer converges in ~one round-trip rather
    // than one interval.
    let node_client = manager.clients.node.clone();
    let _ignored = ctx.spawn(
        async move {
            if let Err(err) = node_client.sync(Some(&context_id), Some(&peer_id)).await {
                warn!(%context_id, %peer_id, ?err, "Failed to auto-sync after context subscription");
            }
        }
        .into_actor(manager),
    );
}

pub(super) fn handle_unsubscribed(
    manager: &mut NodeManager,
    peer_id: libp2p::PeerId,
    topic: libp2p::gossipsub::TopicHash,
) {
    manager
        .clients
        .node
        .record_peer_unsubscribed(&peer_id, &topic);

    let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
        return;
    };

    info!(
        "Peer '{}' unsubscribed from context '{}'",
        peer_id, context_id
    );
}
