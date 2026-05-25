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
            // #2319: surface divergence as a metric (`sync_root_hash_divergence_detected_total`)
            // so vmagent can alert on the rate without grepping logs —
            // with the determinism fixes this should stay near zero.
            let _new = manager.divergence_detected.inc();
            error!(
                %context_id,
                ?source,
                our_hash = ?our_context.root_hash,
                their_hash = ?their_root_hash,
                dag_heads = ?their_dag_heads,
                "DIVERGENCE DETECTED: Same DAG heads but different root hash!"
            );
            // #2319 triage aid — dump ROOT's children list so a future
            // flake can be triaged by diffing the two peers' dumps to
            // pinpoint the divergent subtree. Without this, the only
            // observable signal is the two opaque root hashes and the
            // remaining investigation requires re-running with more
            // logging. Keep the dump rate-bounded by the heartbeat
            // cadence (one DIVERGENCE event per peer per heartbeat).
            //
            // Dump ROOT's own_hash/full_hash first so the diff order
            // matches the analysis flow: identical children + different
            // own_hash points at ROOT-entity write-path divergence
            // (the pattern we saw on PR #2472 attempt 1, all 20
            // children matched).
            match context_client.dump_root_self(&context_id) {
                Ok(Some(self_dump)) => {
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        root_own_hash = %hex::encode(self_dump.own_hash),
                        root_full_hash = %hex::encode(self_dump.full_hash),
                        root_entry_bytes_hash = ?self_dump.entry_bytes_hash.map(hex::encode),
                        root_entry_bytes_len = self_dump.entry_bytes_len,
                        children_count = self_dump.children_count,
                        "DIVERGENCE DUMP: ROOT self"
                    );
                }
                Ok(None) => {
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        "DIVERGENCE DUMP: ROOT self — no index entry"
                    );
                }
                Err(e) => {
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        error = %e,
                        "DIVERGENCE DUMP: failed to read ROOT self"
                    );
                }
            }
            match context_client.dump_root_children(&context_id) {
                Ok(children) => {
                    // Emit one event per child so log search/filter
                    // tools can group by `entity_id`. The whole list
                    // could also be emitted as `?children` but
                    // structured single-row events grep + diff better.
                    for c in &children {
                        warn!(
                            target: "sync::divergence_dump",
                            %context_id,
                            ?source,
                            entity_id = %hex::encode(c.id),
                            merkle_hash = %hex::encode(c.merkle_hash),
                            created_at = c.created_at,
                            updated_at = c.updated_at,
                            crdt_type = ?c.crdt_type,
                            field_name = ?c.field_name,
                            "DIVERGENCE DUMP: ROOT child entry"
                        );
                    }
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        child_count = children.len(),
                        "DIVERGENCE DUMP: ROOT children list emitted"
                    );
                }
                Err(e) => {
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        error = %e,
                        "DIVERGENCE DUMP: failed to read ROOT children list"
                    );
                }
            }
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
