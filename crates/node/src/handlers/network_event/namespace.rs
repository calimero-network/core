use std::collections::HashSet;

use actix::{AsyncContext, WrapFuture};
use calimero_context_client::local_governance::SignedNamespaceOp;
use calimero_node_primitives::sync::{BroadcastMessage, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES};
use tracing::{debug, info, warn};

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
    let op_for_delivery = op.clone();
    let _ignored = ctx.spawn(
        async move {
            if let Err(err) = context_client.apply_signed_namespace_op(op).await {
                warn!(?err, %source, "failed to apply namespace governance delta");
                return;
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
    let context_client = this.clients.context.clone();
    let network_client = this.managers.sync.network_client.clone();
    let sync_timeout = this.managers.sync.sync_config.timeout;

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
                        let payload = BroadcastMessage::NamespaceGovernanceDelta {
                            namespace_id,
                            delta_id: *delta_id,
                            parent_ids: vec![],
                            payload: value.skeleton_bytes,
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

            let Ok(mut stream) = network_client.open_stream(source).await else {
                debug!(
                    %source,
                    "failed to open stream for namespace delta catch-up"
                );
                return;
            };

            let msg = calimero_node_primitives::sync::StreamMessage::Init {
                context_id: calimero_primitives::context::ContextId::from([0u8; 32]),
                party_id: calimero_primitives::identity::PublicKey::from([0u8; 32]),
                payload: calimero_node_primitives::sync::InitPayload::NamespaceBackfillRequest {
                    namespace_id,
                    delta_ids: we_need,
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
                        calimero_node_primitives::sync::MessagePayload::NamespaceBackfillResponse {
                            deltas,
                        },
                    ..
                })) => {
                    for (delta_id, op_bytes) in deltas {
                        if let Ok(op) = borsh::from_slice::<SignedNamespaceOp>(&op_bytes) {
                            if let Err(err) = context_client.apply_signed_namespace_op(op).await {
                                warn!(
                                    %source,
                                    namespace_id = %hex::encode(namespace_id),
                                    delta_id = %hex::encode(delta_id),
                                    ?err,
                                    "failed to apply namespace backfill op from heartbeat catch-up"
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
        .into_actor(this),
    );
}
