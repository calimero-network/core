use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::messages::NetworkEvent;
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
            info!(%peer_id, group_id=%hex, "Peer subscribed to group topic, triggering sync");
            let context_client = manager.clients.context.clone();
            let _ignored = ctx.spawn(
                async move {
                    use calimero_context_client::group::SyncGroupRequest;
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
                }
                .into_actor(manager),
            );

            let context_client_alias = manager.clients.context.clone();
            let _ignored_alias = ctx.spawn(
                async move {
                    use calimero_context_client::group::BroadcastGroupAliasesRequest;
                    use calimero_context_config::types::ContextGroupId;

                    let group_id = ContextGroupId::from(bytes);
                    if let Err(err) = context_client_alias
                        .broadcast_group_aliases(BroadcastGroupAliasesRequest { group_id })
                        .await
                    {
                        warn!(
                            ?err,
                            "Failed to re-broadcast group aliases after peer subscription"
                        );
                    }
                }
                .into_actor(manager),
            );

            let context_client_local_state = manager.clients.context.clone();
            let _ignored_local_state = ctx.spawn(
                async move {
                    use calimero_context_client::group::BroadcastGroupLocalStateRequest;
                    use calimero_context_config::types::ContextGroupId;

                    let group_id = ContextGroupId::from(bytes);
                    if let Err(err) = context_client_local_state
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

    let Ok(context_id): Result<ContextId, _> = topic_str.parse() else {
        return;
    };

    if !manager
        .clients
        .context
        .has_context(&context_id)
        .unwrap_or_default()
    {
        debug!(
            %context_id,
            %peer_id,
            "Observed subscription to unknown context, ignoring.."
        );
        return;
    }

    info!("Peer '{}' subscribed to context '{}'", peer_id, context_id);
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
