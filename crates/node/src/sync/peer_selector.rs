use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::ContextId;
use eyre::{bail, Result};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::Rng;
use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

use crate::utils::choose_stream;

use super::config::SyncConfig;
use super::stream::{recv, send};

#[derive(Clone, Debug)]
pub(crate) struct PeerSelector {
    sync_config: SyncConfig,
    network_client: NetworkClient,
    context_client: ContextClient,
}

impl PeerSelector {
    pub(crate) fn new(
        sync_config: SyncConfig,
        network_client: NetworkClient,
        context_client: ContextClient,
    ) -> Self {
        Self {
            sync_config,
            network_client,
            context_client,
        }
    }

    pub(crate) async fn candidate_peers(
        &self,
        context_id: ContextId,
        requested_peer: Option<PeerId>,
    ) -> Result<Vec<PeerId>> {
        if let Some(peer) = requested_peer {
            return Ok(vec![peer]);
        }

        let mut peers = Vec::new();
        for attempt in 1..=3 {
            peers = self
                .network_client
                .mesh_peers(TopicHash::from_raw(context_id))
                .await;

            if !peers.is_empty() {
                break;
            }

            if attempt < 3 {
                debug!(
                    %context_id,
                    attempt,
                    "No peers found yet, mesh may still be forming, retrying..."
                );
                sleep(Duration::from_millis(500)).await;
            }
        }

        if peers.is_empty() {
            bail!("No peers to sync with for context {}", context_id);
        }

        let context = self
            .context_client
            .get_context(&context_id)?
            .ok_or_else(|| eyre::eyre!("Context not found: {}", context_id))?;

        let is_uninitialized = *context.root_hash == [0; 32];

        let mut rng = rand::thread_rng();

        if is_uninitialized {
            info!(
                %context_id,
                peer_count = peers.len(),
                "Node is uninitialized, selecting peer with state for bootstrapping"
            );
            match self.find_peer_with_state(context_id, &peers).await {
                Ok(peer_with_state) => {
                    peers.retain(|peer| peer != &peer_with_state);
                    peers.shuffle(&mut rng);
                    let mut ordered = Vec::with_capacity(peers.len() + 1);
                    ordered.push(peer_with_state);
                    ordered.extend(peers);
                    return Ok(ordered);
                }
                Err(err) => {
                    warn!(
                        %context_id,
                        error = %err,
                        "Failed to find peer with state, falling back to random selection"
                    );
                }
            }
        }

        peers.shuffle(&mut rng);
        Ok(peers)
    }

    async fn find_peer_with_state(
        &self,
        context_id: ContextId,
        peers: &[PeerId],
    ) -> Result<PeerId> {
        use calimero_node_primitives::sync::MessagePayload;

        let identities = self
            .context_client
            .get_context_members(&context_id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut rand::thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context_id);
        };

        for peer_id in peers {
            debug!(%context_id, %peer_id, "Querying peer for state");

            let stream_result = self.network_client.open_stream(*peer_id).await;
            let mut stream = match stream_result {
                Ok(stream) => stream,
                Err(err) => {
                    debug!(%context_id, %peer_id, error = %err, "Failed to open stream to peer");
                    continue;
                }
            };

            let request_msg = StreamMessage::Init {
                context_id,
                party_id: our_identity,
                payload: InitPayload::DagHeadsRequest { context_id },
                next_nonce: rand::thread_rng().gen(),
            };

            if let Err(err) = send(&mut stream, &request_msg, None).await {
                debug!(%context_id, %peer_id, error = %err, "Failed to send DAG heads request");
                continue;
            }

            let timeout_budget = self.sync_config.timeout / 6;

            let response = match recv(&mut stream, None, timeout_budget).await {
                Ok(Some(resp)) => resp,
                Ok(None) => {
                    debug!(%context_id, %peer_id, "No response from peer");
                    continue;
                }
                Err(err) => {
                    debug!(%context_id, %peer_id, error = %err, "Failed to receive response");
                    continue;
                }
            };

            if let StreamMessage::Message {
                payload:
                    MessagePayload::DagHeadsResponse {
                        dag_heads,
                        root_hash,
                    },
                ..
            } = response
            {
                let has_state = *root_hash != [0; 32];

                debug!(
                    %context_id,
                    %peer_id,
                    heads_count = dag_heads.len(),
                    %root_hash,
                    has_state,
                    "Received DAG heads from peer"
                );

                if has_state {
                    info!(
                        %context_id,
                        %peer_id,
                        heads_count = dag_heads.len(),
                        %root_hash,
                        "Found peer with state for bootstrapping"
                    );
                    return Ok(*peer_id);
                }
            }
        }

        bail!("No peers with state found for context {}", context_id)
    }
}
