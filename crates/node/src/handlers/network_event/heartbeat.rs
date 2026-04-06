use std::collections::HashSet;

use actix::{AsyncContext, WrapFuture};
use calimero_primitives::context::ContextId;
use tracing::{debug, error, info, warn};

use crate::NodeManager;

pub(super) fn handle_hash_heartbeat(
    manager: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    context_id: ContextId,
    their_root_hash: calimero_primitives::hash::Hash,
    their_dag_heads: Vec<[u8; 32]>,
) {
    let context_client = manager.clients.context.clone();

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
            return;
        }

        if our_context.root_hash != their_root_hash {
            let heads_we_dont_have: Vec<_> = their_heads_set.difference(&our_heads_set).collect();
            if heads_we_dont_have.is_empty() {
                debug!(
                    %context_id,
                    ?source,
                    our_heads_count = our_context.dag_heads.len(),
                    their_heads_count = their_dag_heads.len(),
                    "Different root hash (peer is behind or concurrent updates)"
                );
                return;
            }

            info!(
                %context_id,
                ?source,
                our_heads_count = our_context.dag_heads.len(),
                their_heads_count = their_dag_heads.len(),
                missing_count = heads_we_dont_have.len(),
                "Peer has DAG heads we don't have - triggering sync"
            );

            let node_client = manager.clients.node.clone();
            let _ignored = ctx.spawn(
                async move {
                    if let Err(e) = node_client.sync(Some(&context_id), None).await {
                        warn!(%context_id, ?e, "Failed to trigger sync from heartbeat");
                    }
                }
                .into_actor(manager),
            );
        }
    }
}
