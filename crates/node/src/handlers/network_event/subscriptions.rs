use actix::{AsyncContext, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::MetaRepository;
use calimero_primitives::context::ContextId;
use tracing::{debug, info, warn};

use crate::NodeManager;

pub(super) fn handle_subscribed(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    peer_id: libp2p::PeerId,
    topic: libp2p::gossipsub::TopicHash,
) {
    let topic_str = topic.as_str();

    // Check for group topic: "group/<hex32>"
    if let Some(hex) = topic_str.strip_prefix("group/") {
        let mut bytes = [0u8; 32];
        if hex::decode_to_slice(hex, &mut bytes).is_ok() {
            let group_id = ContextGroupId::from(bytes);
            match MetaRepository::new(manager.clients.context.datastore()).load(&group_id) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    debug!(%peer_id, group_id=%hex, "Observed subscription to unknown group, ignoring..");
                    return;
                }
                Err(err) => {
                    warn!(%peer_id, group_id=%hex, %err, "group lookup failed while handling subscription; ignoring");
                    return;
                }
            }
            info!(%peer_id, group_id=%hex, "Peer subscribed to group topic, triggering sync");
            let context_client = manager.clients.context.clone();
            let _ignored = ctx.spawn(
                async move {
                    use calimero_context_client::group::{
                        BroadcastGroupLocalStateRequest, SyncGroupRequest,
                    };

                    if let Err(err) = context_client
                        .sync_group(SyncGroupRequest { group_id })
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

pub(super) fn handle_unsubscribed(peer_id: libp2p::PeerId, topic: libp2p::gossipsub::TopicHash) {
    let Ok(context_id): Result<ContextId, _> = topic.as_str().parse() else {
        return;
    };

    info!(
        "Peer '{}' unsubscribed from context '{}'",
        peer_id, context_id
    );
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use calimero_context_config::types::ContextGroupId;
    use calimero_governance_store::{placeholder_admin_identity, MetaRepository};
    use calimero_network_primitives::messages::NetworkEvent;
    use calimero_store::key::{GroupMetaValue, GroupTarget};
    use libp2p::gossipsub::TopicHash;
    use libp2p::PeerId;
    use serial_test::serial;
    use tokio::time::{sleep, timeout};

    use crate::test_node_harness::{boot_test_node, TestNode};

    async fn subscribe_to_group(node: &TestNode, group: [u8; 32]) {
        node.node_addr
            .send(NetworkEvent::Subscribed {
                peer_id: PeerId::random(),
                topic: TopicHash::from_raw(format!("group/{}", hex::encode(group))),
            })
            .await
            .expect("deliver Subscribed to the node actor");
    }

    /// A peer subscribing to the topic of a group this node holds starts a group
    /// sync; one subscribing to a group it does not hold starts none.
    #[actix::test]
    #[serial(boot_test_node)]
    async fn a_subscription_to_an_unknown_group_starts_no_sync() {
        let node = boot_test_node().await;
        let syncs = || node.sync_group_requests.load(Ordering::SeqCst);

        let known = [0x5F; 32];
        let admin = placeholder_admin_identity();
        MetaRepository::new(&node.store)
            .save(
                &ContextGroupId::from(known),
                &GroupMetaValue {
                    target: GroupTarget::default(),
                    created_at: 0,
                    admin_identity: admin,
                    owner_identity: admin,
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("save the group meta");
        subscribe_to_group(&node, known).await;
        timeout(Duration::from_secs(5), async {
            while syncs() == 0 {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("precondition: a known group's subscription starts a sync");

        subscribe_to_group(&node, [0x5E; 32]).await;
        sleep(Duration::from_millis(200)).await;
        assert_eq!(syncs(), 1, "an unknown group's subscription starts no sync");
    }

    /// A `Subscribed` event alone counts for nothing: the count is what the swarm
    /// lists, which here is no one, as after a peer disconnects without unsubscribing.
    #[actix::test]
    #[serial(boot_test_node)]
    async fn a_subscriber_the_swarm_does_not_list_is_not_counted() {
        let node = boot_test_node().await;
        let namespace = TopicHash::from_raw(format!("ns/{}", hex::encode([0x42u8; 32])));
        let foreign = TopicHash::from_raw("not-a-topic-this-node-uses");
        for topic in [&namespace, &foreign] {
            node.node_addr
                .send(NetworkEvent::Subscribed {
                    peer_id: PeerId::random(),
                    topic: topic.clone(),
                })
                .await
                .expect("deliver Subscribed to the node actor");
        }

        assert_eq!(node.node_client.known_subscribers(&namespace).await, 0);
        assert_eq!(node.node_client.known_subscribers(&foreign).await, 0);
    }
}
