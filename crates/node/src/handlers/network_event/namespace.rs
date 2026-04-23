use std::collections::HashSet;

use actix::{AsyncContext, WrapFuture};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::sync::{BroadcastMessage, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES};
use tracing::{debug, info, warn};

use crate::sync::parent_pull::{NextPeer, ParentPullBudget};
use crate::NodeManager;

pub(super) fn handle_namespace_governance_delta(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    namespace_id: [u8; 32],
    payload: Vec<u8>,
) {
    if payload.len() > MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES {
        warn!(
            len = payload.len(),
            "oversized NamespaceGovernanceDelta payload"
        );
        return;
    }

    let op: SignedNamespaceOp = match borsh::from_slice(&payload) {
        Ok(op) => op,
        Err(err) => {
            warn!(%err, "failed to decode NamespaceGovernanceDelta payload");
            return;
        }
    };

    if op.namespace_id != namespace_id {
        warn!("NamespaceGovernanceDelta namespace_id mismatch with topic");
        return;
    }

    if let Err(err) = op.verify_signature() {
        warn!(%err, "NamespaceGovernanceDelta signature verification failed");
        return;
    }

    let context_client = this.clients.context.clone();
    let node_client = this.clients.node.clone();
    let network_client = this.managers.sync.network_client.clone();
    let sync_timeout = this.managers.sync.sync_config.timeout;
    let pull_budget_max_peers = this.managers.sync.sync_config.parent_pull_additional_peers;
    let pull_budget_duration = this.managers.sync.sync_config.parent_pull_budget;
    let op_for_delivery = op.clone();

    let _ignored = ctx.spawn(
        async move {
            let apply_outcome = context_client.apply_signed_namespace_op(op).await;
            let applied = match apply_outcome {
                Ok(applied) => applied,
                Err(err) => {
                    warn!(?err, %source, "failed to apply namespace governance delta");
                    return;
                }
            };

            // Proactive backfill (#2198): if the op went pending (missing
            // parents), don't wait for the next periodic namespace heartbeat.
            // Immediately fetch the namespace DAG from the gossip source so
            // downstream checks like join_context's membership read see a
            // converged state within sub-second latency. If the first peer
            // cannot resolve everything, fall through to cross-peer retry.
            if !applied {
                debug!(
                    %source,
                    namespace_id = %hex::encode(namespace_id),
                    "gossip governance op is pending; triggering proactive backfill"
                );
                resolve_namespace_pending(
                    &context_client,
                    &network_client,
                    source,
                    namespace_id,
                    sync_timeout,
                    pull_budget_max_peers,
                    pull_budget_duration,
                )
                .await;
            }

            crate::key_delivery::maybe_publish_key_delivery(
                &context_client,
                &node_client,
                &op_for_delivery,
            )
            .await;
        }
        .into_actor(this),
    );
}

pub(super) fn handle_namespace_state_heartbeat(
    this: &mut NodeManager,
    ctx: &mut actix::Context<NodeManager>,
    source: libp2p::PeerId,
    namespace_id: [u8; 32],
    peer_heads: Vec<[u8; 32]>,
) {
    // Cap peer-supplied heads to prevent DoS via oversized heartbeat.
    const MAX_PEER_HEADS: usize = 256;
    if peer_heads.len() > MAX_PEER_HEADS {
        warn!(
            %source,
            heads = peer_heads.len(),
            "Namespace heartbeat exceeds max peer heads, ignoring"
        );
        return;
    }

    let context_client = this.clients.context.clone();
    let network_client = this.managers.sync.network_client.clone();
    let sync_timeout = this.managers.sync.sync_config.timeout;
    let pull_budget_max_peers = this.managers.sync.sync_config.parent_pull_additional_peers;
    let pull_budget_duration = this.managers.sync.sync_config.parent_pull_budget;

    let _ignored = ctx.spawn(
        async move {
            let store = context_client.datastore_handle().into_inner();
            let ns_head_key = calimero_store::key::NamespaceGovHead::new(namespace_id);
            let handle = store.handle();
            let local_heads: HashSet<[u8; 32]> = match handle.get(&ns_head_key) {
                Ok(Some(h)) => h.dag_heads.into_iter().collect(),
                _ => HashSet::new(),
            };
            drop(handle);

            let we_need: Vec<[u8; 32]> = peer_heads
                .iter()
                .filter(|h| !local_heads.contains(*h))
                .copied()
                .collect();

            let peer_head_set: HashSet<[u8; 32]> = peer_heads.iter().copied().collect();
            let peer_needs: Vec<[u8; 32]> = local_heads
                .iter()
                .filter(|h| !peer_head_set.contains(*h))
                .copied()
                .collect();

            if !peer_needs.is_empty() {
                let store_inner = context_client.datastore_handle().into_inner();
                let handle_inner = store_inner.handle();
                for delta_id in &peer_needs {
                    let key = calimero_store::key::NamespaceGovOp::new(namespace_id, *delta_id);
                    if let Ok(Some(value)) = handle_inner.get(&key) {
                        let Some(signed_bytes) =
                            crate::sync::helpers::extract_signed_op_bytes(&value.skeleton_bytes)
                        else {
                            continue;
                        };
                        let payload = BroadcastMessage::NamespaceGovernanceDelta {
                            namespace_id,
                            delta_id: *delta_id,
                            parent_ids: vec![],
                            payload: signed_bytes,
                        };
                        if let Ok(bytes) = borsh::to_vec(&payload) {
                            let topic = libp2p::gossipsub::TopicHash::from_raw(format!(
                                "ns/{}",
                                hex::encode(namespace_id)
                            ));
                            let _ = network_client.publish(topic, bytes).await;
                        }
                    }
                }
            }

            if we_need.is_empty() {
                return;
            }

            info!(
                namespace_id = %hex::encode(namespace_id),
                missing = we_need.len(),
                %source,
                "namespace heartbeat divergence: requesting missing deltas"
            );

            // First attempt: the peer that advertised its heads.
            fetch_and_apply_namespace_backfill(
                &context_client,
                &network_client,
                source,
                namespace_id,
                we_need,
                sync_timeout,
            )
            .await;

            // Cross-peer fallback (#2198): if the first peer did not fully
            // resolve our pending chain, iterate other namespace-mesh peers
            // until the DAG drains or the budget is exhausted.
            resolve_namespace_pending(
                &context_client,
                &network_client,
                source,
                namespace_id,
                sync_timeout,
                pull_budget_max_peers,
                pull_budget_duration,
            )
            .await;
        }
        .into_actor(this),
    );
}

/// Iterate other namespace-mesh peers asking for backfill until the local
/// governance DAG has no more pending ops for this namespace, or the retry
/// budget is exhausted.
///
/// Uses empty-body `NamespaceBackfillRequest` (semantics: "give me everything
/// for this namespace", per `handle_namespace_backfill_request`) because
/// callers don't know which specific ancestor ids are still missing — the
/// pending chain can be arbitrarily deep, and the responder caps at
/// `MAX_BACKFILL_OPS` per response anyway.
async fn resolve_namespace_pending(
    context_client: &calimero_context_client::client::ContextClient,
    network_client: &NetworkClient,
    initial_peer: libp2p::PeerId,
    namespace_id: [u8; 32],
    sync_timeout: tokio::time::Duration,
    max_additional_peers: usize,
    budget: tokio::time::Duration,
) {
    let topic = libp2p::gossipsub::TopicHash::from_raw(format!("ns/{}", hex::encode(namespace_id)));
    let mut mesh_peers = network_client.mesh_peers(topic.clone()).await;
    let mut scheduler = ParentPullBudget::new(initial_peer, max_additional_peers, budget);

    loop {
        if !namespace_has_pending(context_client, namespace_id).await {
            break;
        }

        let next_peer = match scheduler.next(&mesh_peers) {
            NextPeer::Peer(p) => p,
            NextPeer::RefetchMesh => {
                mesh_peers = network_client.mesh_peers(topic.clone()).await;
                scheduler.record_refetch();
                match scheduler.next(&mesh_peers) {
                    NextPeer::Peer(p) => p,
                    other => {
                        debug!(
                            namespace_id = %hex::encode(namespace_id),
                            ?other,
                            "no additional ns mesh peers for parent pull"
                        );
                        break;
                    }
                }
            }
            NextPeer::BudgetExhausted => {
                warn!(
                    namespace_id = %hex::encode(namespace_id),
                    "namespace parent-pull budget exhausted"
                );
                break;
            }
            NextPeer::MaxPeersReached | NextPeer::NoMorePeers => break,
        };

        scheduler.record_attempt(next_peer);
        info!(
            namespace_id = %hex::encode(namespace_id),
            ?next_peer,
            attempt = scheduler.attempts(),
            "retrying namespace backfill against additional mesh peer"
        );

        fetch_and_apply_namespace_backfill(
            context_client,
            network_client,
            next_peer,
            namespace_id,
            Vec::new(),
            sync_timeout,
        )
        .await;
    }
}

async fn fetch_and_apply_namespace_backfill(
    context_client: &calimero_context_client::client::ContextClient,
    network_client: &NetworkClient,
    peer: libp2p::PeerId,
    namespace_id: [u8; 32],
    delta_ids: Vec<[u8; 32]>,
    sync_timeout: tokio::time::Duration,
) {
    let Ok(mut stream) = network_client.open_stream(peer).await else {
        debug!(
            %peer,
            "failed to open stream for namespace backfill"
        );
        return;
    };

    let msg = calimero_node_primitives::sync::StreamMessage::Init {
        context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
        party_id: calimero_primitives::identity::PublicKey::from([0u8; 32]),
        payload: calimero_node_primitives::sync::InitPayload::NamespaceBackfillRequest {
            namespace_id,
            delta_ids,
        },
        next_nonce: {
            use rand::Rng;
            rand::thread_rng().gen()
        },
    };

    if let Err(err) = crate::sync::stream::send(&mut stream, &msg, None).await {
        debug!(%err, "failed to send NamespaceBackfillRequest");
        return;
    }

    match crate::sync::stream::recv(&mut stream, None, sync_timeout).await {
        Ok(Some(calimero_node_primitives::sync::StreamMessage::Message {
            payload:
                calimero_node_primitives::sync::MessagePayload::NamespaceBackfillResponse { deltas },
            ..
        })) => {
            for (delta_id, op_bytes) in deltas {
                if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(&op_bytes) {
                    if let Err(err) = context_client.apply_signed_namespace_op(op).await {
                        warn!(
                            %peer,
                            namespace_id = %hex::encode(namespace_id),
                            delta_id = %hex::encode(delta_id),
                            ?err,
                            "failed to apply namespace backfill op"
                        );
                    }
                }
            }
        }
        _ => {
            debug!("unexpected response to NamespaceBackfillRequest");
        }
    }
}

/// Returns `true` if this node's governance DAG has ops whose parents are
/// not yet local (the pending queue is non-empty).
async fn namespace_has_pending(
    context_client: &calimero_context_client::client::ContextClient,
    namespace_id: [u8; 32],
) -> bool {
    context_client
        .namespace_pending_op_count(namespace_id)
        .await
        .unwrap_or(0)
        > 0
}
