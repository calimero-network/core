use std::collections::HashSet;

use actix::{AsyncContext, WrapFuture};
use calimero_network_primitives::messages::Message as NetworkMessage;
use calimero_node_primitives::sync::broadcast::Message as BroadcastMessage;
use tracing::{debug, error, info, warn};

use crate::NodeManager;

/// Handles inbound gossipsub messages (broadcast channel).
pub fn handle_message(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    message: NetworkMessage,
) {
    let Some(source) = message.source else {
        warn!(?message, "Received message without source");
        return;
    };

    let message = match borsh::from_slice::<BroadcastMessage<'_>>(&message.data) {
        Ok(message) => message,
        Err(err) => {
            debug!(?err, ?message, "Failed to deserialize message");
            return;
        }
    };

    #[expect(clippy::match_same_arms, reason = "clearer separation")]
    match message {
        BroadcastMessage::StateDelta {
            context_id,
            author_id,
            delta_id,
            parent_ids,
            hlc,
            root_hash,
            artifact,
            nonce,
            events,
        } => {
            info!(
                %context_id,
                %author_id,
                delta_id = ?delta_id,
                parent_count = parent_ids.len(),
                has_events = events.is_some(),
                "Matched StateDelta message"
            );

            let node_clients = node_manager.clients.clone();
            let node_state = node_manager.state.clone();
            let network_client = node_manager.managers.sync.network_client.clone();
            let sync_config_timeout = node_manager.managers.sync.sync_config.timeout;

            let _ignored = ctx.spawn(
                async move {
                    if let Err(err) = crate::comms::broadcast::state_delta::handle_state_delta(
                        node_clients,
                        node_state,
                        network_client,
                        sync_config_timeout,
                        source,
                        context_id,
                        author_id,
                        delta_id,
                        parent_ids,
                        hlc,
                        root_hash,
                        artifact.into_owned(),
                        nonce,
                        events.map(|e| e.into_owned()),
                    )
                    .await
                    {
                        warn!(?err, "Failed to handle state delta");
                    }
                }
                .into_actor(node_manager),
            );
        }
        BroadcastMessage::HashHeartbeat {
            context_id,
            root_hash: their_root_hash,
            dag_heads: their_dag_heads,
        } => {
            handle_hash_heartbeat(
                node_manager,
                ctx,
                context_id,
                their_root_hash,
                their_dag_heads,
                source,
            );
        }
        other => {
            warn!(
                ?other,
                "Received unexpected broadcast message type (not StateDelta or HashHeartbeat)"
            );
        }
    }
}

fn handle_hash_heartbeat(
    node_manager: &mut NodeManager,
    ctx: &mut <NodeManager as actix::Actor>::Context,
    context_id: calimero_primitives::context::ContextId,
    their_root_hash: calimero_primitives::hash::Hash,
    their_dag_heads: Vec<[u8; 32]>,
    source: libp2p::PeerId,
) {
    let context_client = node_manager.clients.context.clone();

    if let Ok(Some(our_context)) = context_client.get_context(&context_id) {
        let our_heads_set: HashSet<_> = our_context.dag_heads.iter().collect();
        let their_heads_set: HashSet<_> = their_dag_heads.iter().collect();

        if our_heads_set == their_heads_set && our_context.root_hash != their_root_hash {
            error!(
                %context_id,
                ?source,
                our_hash = ?our_context.root_hash,
                their_hash = ?their_root_hash,
                dag_heads = ?their_dag_heads,
                "DIVERGENCE DETECTED: Same DAG heads but different root hash!"
            );

            warn!(
                %context_id,
                ?source,
                their_heads = ?their_dag_heads,
                "Divergence detected - periodic sync will recover"
            );
        } else if our_context.root_hash != their_root_hash {
            let heads_we_dont_have: Vec<_> = their_heads_set.difference(&our_heads_set).collect();

            if !heads_we_dont_have.is_empty() {
                info!(
                    %context_id,
                    ?source,
                    our_heads_count = our_context.dag_heads.len(),
                    their_heads_count = their_dag_heads.len(),
                    missing_count = heads_we_dont_have.len(),
                    "Peer has DAG heads we don't have - triggering sync"
                );

                let node_client = node_manager.clients.node.clone();
                let context_id_cloned = context_id;
                ctx.spawn(
                    async move {
                        if let Err(e) = node_client.sync(Some(&context_id_cloned), None).await {
                            warn!(context_id = %context_id_cloned, ?e, "Failed to trigger sync from heartbeat");
                        }
                    }
                    .into_actor(node_manager),
                );
            } else {
                debug!(
                    %context_id,
                    ?source,
                    our_heads_count = our_context.dag_heads.len(),
                    their_heads_count = their_dag_heads.len(),
                    "Different root hash (peer is behind or concurrent updates)"
                );
            }
        }
    }
}
