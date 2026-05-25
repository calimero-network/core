use std::collections::HashSet;

use actix::{AsyncContext, WrapFuture};
use calimero_primitives::context::ContextId;
use tracing::{debug, error, info, warn};

use crate::NodeManager;

/// Cap on per-child `warn!` events emitted from the divergence dump.
/// A wide context (e.g. UnorderedMap with hundreds of entries) could
/// otherwise produce a log burst that overwhelms aggregation
/// pipelines on every divergence tick. The summary row below the
/// loop reports the full count regardless.
const MAX_DUMP_CHILDREN: usize = 64;

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
            // #2319 — before reporting a real divergence, reconcile the
            // cached `ContextMeta.root_hash` with the live index. The
            // cache is populated from the WASM-returned root_hash at the
            // end of each method execution; a concurrent sync apply
            // (HashComparison `EntityPush`, level-sync leaf push, etc.)
            // can advance the actual index right after WASM returns but
            // before the cache is written, leaving the cache pointing
            // at a pre-recalc full_hash while the index has already
            // moved on. The two nodes still converge in storage, but
            // the heartbeat sees the stale caches and fires
            // DIVERGENCE.
            //
            // Re-read the live full_hash from the index. If it matches
            // either the peer's hash or our prior cache, refresh the
            // cache and skip the false-positive divergence event; only
            // a *post-refresh* hash mismatch is a real divergence.
            //
            // Residual TOCTOU: a concurrent apply landing between
            // `compute_root_hash` and `force_root_hash` can leave the
            // cache transiently stale again. This doesn't eliminate
            // the false-positive class — it narrows the window from
            // "until the next WASM execution" to "until the next
            // heartbeat tick" (also when the next ContextMeta write
            // happens). The next heartbeat self-heals via this same
            // branch, so we accept the residual window rather than
            // double-reading.
            let our_hash = match context_client.compute_root_hash(&context_id) {
                Ok(live) => {
                    let live_hash = calimero_primitives::hash::Hash::from(live);
                    if live_hash != our_context.root_hash {
                        debug!(
                            %context_id,
                            ?source,
                            cached_hash = ?our_context.root_hash,
                            live_hash = ?live_hash,
                            "Heartbeat divergence: cache stale vs live index, refreshing"
                        );
                        if let Err(err) = context_client.force_root_hash(&context_id, live_hash) {
                            // `warn`, not `debug`: a single failure here
                            // is usually a benign concurrent context
                            // delete, but a *persistent* failure leaves
                            // the cache stale and produces a stream of
                            // false-positive DIVERGENCE events on every
                            // heartbeat. Surface it where ops can see it
                            // without log filtering.
                            warn!(
                                %context_id,
                                ?source,
                                %err,
                                "Heartbeat divergence: failed to refresh cached root hash"
                            );
                        }
                        if live_hash == their_root_hash {
                            return;
                        }
                    }
                    // Use live_hash as the authoritative "our hash" — the
                    // cached value may be stale (and the refresh above
                    // may itself have failed); the live index is the
                    // truth we want in the divergence log for triage.
                    live_hash
                }
                Err(err) => {
                    debug!(
                        %context_id,
                        ?source,
                        %err,
                        "Heartbeat divergence: failed to read live root hash; using cache"
                    );
                    our_context.root_hash
                }
            };

            // #2319: surface divergence as a metric (`sync_root_hash_divergence_detected_total`)
            // so vmagent can alert on the rate without grepping logs —
            // with the determinism fixes this should stay near zero.
            let _new = manager.divergence_detected.inc();
            error!(
                %context_id,
                ?source,
                our_hash = ?our_hash,
                their_hash = ?their_root_hash,
                dag_heads = ?their_dag_heads,
                "DIVERGENCE DETECTED: Same DAG heads but different root hash!"
            );
            // #2319 triage aid — dump ROOT's self summary + children
            // list so a future flake can be triaged by diffing the two
            // peers' dumps. Without this, the only observable signal
            // is the two opaque root hashes and the remaining
            // investigation requires re-running with more logging.
            // Bounded by the heartbeat cadence (one DIVERGENCE event
            // per peer per heartbeat) and by MAX_DUMP_CHILDREN.
            //
            // Self summary is logged before children so the diff order
            // matches the analysis flow: identical children +
            // different own_hash points at ROOT-entity write-path
            // divergence (the pattern we saw on PR #2472 attempt 1,
            // all 20 children matched).
            match context_client.dump_root(&context_id) {
                Ok(Some((self_dump, children))) => {
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
                    let child_count = children.len();
                    // Emit one event per child so log search/filter
                    // tools can group by `entity_id`. Cap the per-child
                    // emission at MAX_DUMP_CHILDREN — the summary row
                    // below reports the full count regardless.
                    for c in children.iter().take(MAX_DUMP_CHILDREN) {
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
                    if child_count > MAX_DUMP_CHILDREN {
                        warn!(
                            target: "sync::divergence_dump",
                            %context_id,
                            ?source,
                            emitted = MAX_DUMP_CHILDREN,
                            total = child_count,
                            "DIVERGENCE DUMP: ROOT children truncated"
                        );
                    }
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        child_count,
                        "DIVERGENCE DUMP: ROOT children list emitted"
                    );
                }
                Ok(None) => {
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        "DIVERGENCE DUMP: ROOT — no index entry"
                    );
                }
                Err(e) => {
                    warn!(
                        target: "sync::divergence_dump",
                        %context_id,
                        ?source,
                        error = %e,
                        "DIVERGENCE DUMP: failed to read ROOT"
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
