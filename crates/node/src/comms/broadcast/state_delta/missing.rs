use calimero_network_primitives::client::NetworkClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use eyre::Result;
use libp2p::PeerId;
use tracing::{info, warn};

use crate::delta_store::DeltaStore;

pub(super) async fn request_missing_deltas(
    network_client: NetworkClient,
    sync_timeout: std::time::Duration,
    context_id: ContextId,
    missing_ids: Vec<[u8; 32]>,
    source: PeerId,
    our_identity: PublicKey,
    delta_store: DeltaStore,
) -> Result<()> {
    use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};

    let mut stream = network_client.open_stream(source).await?;

    let mut to_fetch = missing_ids;
    let mut fetched_deltas: Vec<(calimero_dag::CausalDelta<Vec<Action>>, [u8; 32])> = Vec::new();
    let mut fetch_count = 0;

    while !to_fetch.is_empty() {
        let current_batch = to_fetch.clone();
        to_fetch.clear();

        for missing_id in current_batch {
            fetch_count += 1;

            info!(
                %context_id,
                delta_id = ?missing_id,
                total_fetched = fetch_count,
                "Requesting missing parent delta from peer"
            );

            let request_msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::DeltaRequest {
                    context_id,
                    delta_id: missing_id,
                },
                next_nonce: {
                    use rand::Rng;
                    rand::thread_rng().gen()
                },
            };

            crate::sync::stream::send(&mut stream, &request_msg, None).await?;

            let timeout_budget = sync_timeout / 3;
            match crate::sync::stream::recv(&mut stream, None, timeout_budget).await? {
                Some(StreamMessage::Message {
                    payload: MessagePayload::DeltaResponse { delta },
                    ..
                }) => {
                    let storage_delta: calimero_storage::delta::CausalDelta =
                        borsh::from_slice(&delta)?;

                    info!(
                        %context_id,
                        delta_id = ?missing_id,
                        action_count = storage_delta.actions.len(),
                        "Received missing parent delta"
                    );

                    let dag_delta = calimero_dag::CausalDelta {
                        id: storage_delta.id,
                        parents: storage_delta.parents.clone(),
                        payload: storage_delta.actions,
                        hlc: storage_delta.hlc,
                        expected_root_hash: storage_delta.expected_root_hash,
                    };

                    fetched_deltas.push((dag_delta, missing_id));

                    for parent_id in &storage_delta.parents {
                        if *parent_id == [0; 32] {
                            continue;
                        }
                        if !delta_store.has_delta(parent_id).await
                            && !to_fetch.contains(parent_id)
                            && !fetched_deltas.iter().any(|(d, _)| d.id == *parent_id)
                        {
                            to_fetch.push(*parent_id);
                        }
                    }
                }
                Some(StreamMessage::Message {
                    payload: MessagePayload::DeltaNotFound,
                    ..
                }) => {
                    warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
                }
                other => {
                    warn!(%context_id, delta_id = ?missing_id, ?other, "Unexpected response to delta request");
                }
            }
        }
    }

    if !fetched_deltas.is_empty() {
        info!(
            %context_id,
            total_fetched = fetched_deltas.len(),
            "Adding fetched deltas to DAG in topological order"
        );

        fetched_deltas.reverse();

        for (dag_delta, delta_id) in fetched_deltas {
            if let Err(e) = delta_store.add_delta(dag_delta).await {
                warn!(?e, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
            }
        }

        if fetch_count > 1000 {
            warn!(
                %context_id,
                total_fetched = fetch_count,
                "Large sync detected - fetched many deltas from peer (context has deep history)"
            );
        }
    }

    Ok(())
}
