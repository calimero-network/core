use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, Result};
use libp2p::PeerId;
use rand::Rng;
use tracing::{debug, error, info, warn};

use crate::delta_store::DeltaStore;
use crate::sync::config::SyncConfig;
use crate::sync::direct::helpers::generate_nonce;
use crate::sync::stream::{recv, send};
use crate::sync::tracking::SyncProtocol;

#[derive(Clone, Debug)]
pub(crate) struct DagBootstrapper {
    sync_config: SyncConfig,
    context_client: ContextClient,
    network_client: NetworkClient,
    node_state: crate::NodeState,
}

impl DagBootstrapper {
    pub(crate) fn new(
        sync_config: SyncConfig,
        context_client: ContextClient,
        network_client: NetworkClient,
        node_state: crate::NodeState,
    ) -> Self {
        Self {
            sync_config,
            context_client,
            network_client,
            node_state,
        }
    }

    pub(crate) async fn catch_up(
        &self,
        context_id: ContextId,
        peer_id: PeerId,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<SyncProtocol> {
        info!(
            %context_id,
            heads_peer = %peer_id,
            "Requesting DAG heads from peer"
        );

        let request = StreamMessage::Init {
            context_id,
            party_id: our_identity,
            payload: InitPayload::DagHeadsRequest { context_id },
            next_nonce: rand::thread_rng().gen(),
        };

        send(stream, &request, None).await?;

        let timeout_budget = self.sync_config.timeout / 6;
        let response = match recv(stream, None, timeout_budget).await? {
            Some(message) => message,
            None => {
                info!(%context_id, %peer_id, "Peer closed stream before responding to DAG heads request");
                return Ok(SyncProtocol::None);
            }
        };

        match response {
            StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            } => {
                info!(
                    %context_id,
                    heads_count = dag_heads.len(),
                    peer_root_hash = %root_hash,
                    "Received DAG heads from peer, requesting deltas"
                );

                if dag_heads.is_empty() && *root_hash != [0; 32] {
                    error!(
                        %context_id,
                        peer_root_hash = %root_hash,
                        "Peer has state but no DAG heads!"
                    );
                    bail!(
                        "Peer has state but no DAG heads (migration issue). \
                         Clear data directories on both nodes and recreate context."
                    );
                }

                if dag_heads.is_empty() {
                    info!(%context_id, "Peer also has no deltas and no state, will try next peer");
                    return Ok(SyncProtocol::None);
                }

                let (delta_store_ref, is_new_store) = {
                    let mut is_new = false;
                    let delta_store = self
                        .node_state
                        .delta_stores
                        .entry(context_id)
                        .or_insert_with(|| {
                            is_new = true;
                            DeltaStore::new(
                                [0u8; 32],
                                self.context_client.clone(),
                                context_id,
                                our_identity,
                            )
                        });

                    (delta_store.clone(), is_new)
                };

                if is_new_store {
                    if let Err(err) = delta_store_ref.load_persisted_deltas().await {
                        warn!(?err, %context_id, "Failed to load persisted deltas, starting with empty DAG");
                    }
                }

                for head_id in &dag_heads {
                    info!(%context_id, head_id = ?head_id, "Requesting DAG head delta from peer");

                    let delta_request = StreamMessage::Init {
                        context_id,
                        party_id: our_identity,
                        payload: InitPayload::DeltaRequest {
                            context_id,
                            delta_id: *head_id,
                        },
                        next_nonce: rand::thread_rng().gen(),
                    };

                    send(stream, &delta_request, None).await?;

                    let delta_response = self.recv_delta(stream).await?;

                    match delta_response {
                        Some(StreamMessage::Message {
                            payload: MessagePayload::DeltaResponse { delta },
                            ..
                        }) => {
                            let storage_delta: calimero_storage::delta::CausalDelta =
                                borsh::from_slice(&delta)?;

                            let dag_delta = calimero_dag::CausalDelta {
                                id: storage_delta.id,
                                parents: storage_delta.parents,
                                payload: storage_delta.actions,
                                hlc: storage_delta.hlc,
                                expected_root_hash: storage_delta.expected_root_hash,
                            };

                            if let Err(err) = delta_store_ref.add_delta(dag_delta).await {
                                warn!(?err, %context_id, head_id = ?head_id, "Failed to add DAG head delta");
                            } else {
                                info!(%context_id, head_id = ?head_id, "Successfully added DAG head delta");
                            }
                        }
                        _ => {
                            warn!(%context_id, head_id = ?head_id, "Unexpected response to delta request");
                        }
                    }
                }

                let missing_result = delta_store_ref.get_missing_parents().await;

                if !missing_result.cascaded_events.is_empty() {
                    info!(
                        %context_id,
                        cascaded_count = missing_result.cascaded_events.len(),
                        "Cascaded deltas from DB load during DAG head sync"
                    );
                }

                if !missing_result.missing_ids.is_empty() {
                    info!(
                        %context_id,
                        missing_count = missing_result.missing_ids.len(),
                        "DAG heads have missing parents, requesting them recursively"
                    );

                    if let Err(err) = self
                        .request_missing_deltas(
                            context_id,
                            missing_result.missing_ids,
                            peer_id,
                            delta_store_ref.clone(),
                            our_identity,
                        )
                        .await
                    {
                        warn!(?err, %context_id, "Failed to request missing parent deltas during DAG catchup");
                    }
                }

                Ok(SyncProtocol::DagCatchup)
            }
            other => {
                warn!(%context_id, ?other, "Unexpected response to DAG heads request, trying next peer");
                Ok(SyncProtocol::None)
            }
        }
    }

    async fn recv_delta(
        &self,
        stream: &mut Stream,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let budget = self.sync_config.timeout / 3;
        recv(stream, None, budget).await
    }

    async fn request_missing_deltas(
        &self,
        context_id: ContextId,
        mut missing_ids: Vec<[u8; 32]>,
        source: PeerId,
        delta_store: DeltaStore,
        our_identity: PublicKey,
    ) -> Result<()> {
        info!(
            %context_id,
            ?source,
            initial_missing_count = missing_ids.len(),
            "Requesting missing parent deltas from peer"
        );

        let mut stream = self.network_client.open_stream(source).await?;
        let mut fetched_deltas: Vec<(
            calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>>,
            [u8; 32],
        )> = Vec::new();
        let mut fetch_count = 0;

        while !missing_ids.is_empty() {
            let current_batch = missing_ids.clone();
            missing_ids.clear();

            for missing_id in current_batch {
                fetch_count += 1;

                match self
                    .request_delta(&context_id, missing_id, &mut stream, our_identity)
                    .await
                {
                    Ok(Some(parent_delta)) => {
                        info!(
                            %context_id,
                            delta_id = ?missing_id,
                            action_count = parent_delta.actions.len(),
                            total_fetched = fetch_count,
                            "Received missing parent delta"
                        );

                        let dag_delta = calimero_dag::CausalDelta {
                            id: parent_delta.id,
                            parents: parent_delta.parents.clone(),
                            payload: parent_delta.actions,
                            hlc: parent_delta.hlc,
                            expected_root_hash: parent_delta.expected_root_hash,
                        };

                        fetched_deltas.push((dag_delta, missing_id));

                        for parent_id in &parent_delta.parents {
                            if *parent_id == [0; 32] {
                                continue;
                            }

                            let already_queued = missing_ids.contains(parent_id)
                                || fetched_deltas
                                    .iter()
                                    .any(|(delta, _)| delta.id == *parent_id);

                            if !already_queued && !delta_store.has_delta(parent_id).await {
                                missing_ids.push(*parent_id);
                            }
                        }
                    }
                    Ok(None) => {
                        warn!(%context_id, delta_id = ?missing_id, "Peer doesn't have requested delta");
                    }
                    Err(err) => {
                        warn!(?err, %context_id, delta_id = ?missing_id, "Failed to request delta");
                        break;
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
                if let Err(err) = delta_store.add_delta(dag_delta).await {
                    warn!(?err, %context_id, delta_id = ?delta_id, "Failed to add fetched delta to DAG");
                }
            }
        }

        if fetch_count > 0 {
            info!(%context_id, total_fetched = fetch_count, "Completed fetching missing delta ancestors");

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

    async fn request_delta(
        &self,
        context_id: &ContextId,
        delta_id: [u8; 32],
        stream: &mut Stream,
        our_identity: PublicKey,
    ) -> Result<Option<calimero_storage::delta::CausalDelta>> {
        info!(%context_id, delta_id = ?delta_id, "Requesting missing delta from peer");

        let msg = StreamMessage::Init {
            context_id: *context_id,
            party_id: our_identity,
            payload: InitPayload::DeltaRequest {
                context_id: *context_id,
                delta_id,
            },
            next_nonce: generate_nonce(),
        };

        send(stream, &msg, None).await?;

        let timeout_budget = self.sync_config.timeout;

        match recv(stream, None, timeout_budget).await? {
            Some(StreamMessage::Message {
                payload: MessagePayload::DeltaResponse { delta },
                ..
            }) => {
                let causal_delta: calimero_storage::delta::CausalDelta = borsh::from_slice(&delta)?;

                if causal_delta.id != delta_id {
                    bail!(
                        "Received delta ID mismatch: requested {:?}, got {:?}",
                        delta_id,
                        causal_delta.id
                    );
                }

                Ok(Some(causal_delta))
            }
            Some(StreamMessage::Message {
                payload: MessagePayload::DeltaNotFound,
                ..
            }) => Ok(None),
            Some(StreamMessage::OpaqueError) => {
                bail!("Peer encountered error processing delta request")
            }
            other => bail!("Unexpected response to delta request: {:?}", other),
        }
    }
}
