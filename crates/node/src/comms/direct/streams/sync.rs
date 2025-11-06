use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::direct::{InitPayload, StreamMessage};
use eyre::{bail, Result};
use rand::thread_rng;
use tracing::error;

use super::BlobResponder;
use crate::comms::direct::{delta_request, key_exchange};
use crate::sync::stream::{recv, send};
use crate::sync::SyncConfig;
use crate::utils::choose_stream;

#[derive(Clone, Debug)]
pub(crate) struct SyncResponder {
    sync_config: SyncConfig,
    context_client: ContextClient,
    node_state: crate::NodeState,
    blob_responder: BlobResponder,
}

impl SyncResponder {
    pub(crate) fn new(
        sync_config: SyncConfig,
        context_client: ContextClient,
        node_state: crate::NodeState,
        blob_responder: BlobResponder,
    ) -> Self {
        Self {
            sync_config,
            context_client,
            node_state,
            blob_responder,
        }
    }

    pub(crate) async fn handle_opened_stream(&self, mut stream: Box<Stream>) {
        loop {
            match self.internal_handle_opened_stream(&mut stream).await {
                Ok(None) => break,
                Ok(Some(())) => {}
                Err(err) => {
                    error!(%err, "Failed to handle stream message");

                    if let Err(err) = send(&mut stream, &StreamMessage::OpaqueError, None).await {
                        error!(%err, "Failed to send error message");
                    }
                }
            }
        }
    }

    async fn internal_handle_opened_stream(&self, stream: &mut Stream) -> Result<Option<()>> {
        let Some(message) = recv(stream, None, self.sync_config.timeout / 3).await? else {
            return Ok(None);
        };

        let (context_id, their_identity, payload, _nonce) = match message {
            StreamMessage::Init {
                context_id,
                party_id,
                payload,
                next_nonce,
                ..
            } => (context_id, party_id, payload, next_nonce),
            unexpected @ (StreamMessage::Message { .. } | StreamMessage::OpaqueError) => {
                bail!("expected initialization handshake, got {:?}", unexpected)
            }
        };

        let Some(context) = self.context_client.get_context(&context_id)? else {
            bail!("context not found: {}", context_id);
        };

        if !self
            .context_client
            .has_member(&context_id, &their_identity)?
        {
            self.context_client
                .sync_context_config(context_id, None)
                .await?;

            if !self
                .context_client
                .has_member(&context_id, &their_identity)?
            {
                bail!(
                    "unknown context member {} in context {}",
                    their_identity,
                    context_id
                );
            }
        }

        let identities = self
            .context_client
            .get_context_members(&context.id, Some(true));

        let Some((our_identity, _)) = choose_stream(identities, &mut thread_rng())
            .await
            .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
        };

        match payload {
            InitPayload::KeyShare => {
                key_exchange::handle_key_share_request(
                    &self.context_client,
                    &context,
                    our_identity,
                    their_identity,
                    stream,
                    self.sync_config.timeout,
                )
                .await?
            }
            InitPayload::BlobShare { blob_id } => {
                self.blob_responder
                    .handle_blob_share_request(
                        &context,
                        our_identity,
                        their_identity,
                        blob_id,
                        stream,
                    )
                    .await?
            }
            InitPayload::DeltaRequest {
                context_id: requested_context_id,
                delta_id,
            } => {
                delta_request::handle_delta_request(
                    &self.context_client,
                    &self.node_state,
                    requested_context_id,
                    delta_id,
                    stream,
                )
                .await?
            }
            InitPayload::DagHeadsRequest {
                context_id: requested_context_id,
            } => {
                delta_request::handle_dag_heads_request(
                    &self.context_client,
                    requested_context_id,
                    stream,
                )
                .await?
            }
        };

        Ok(Some(()))
    }
}
