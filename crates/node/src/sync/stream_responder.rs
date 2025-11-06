use calimero_context_primitives::client::ContextClient;
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{InitPayload, StreamMessage};
use calimero_primitives::context::ContextId;
use eyre::{bail, Result};
use tracing::{error, info};

use super::config::SyncConfig;
use super::stream::{recv, send};
use super::tracking::Sequencer;

#[derive(Clone, Debug)]
pub(crate) struct StreamResponder {
    sync_config: SyncConfig,
    node_client: NodeClient,
    context_client: ContextClient,
    node_state: crate::NodeState,
}

impl StreamResponder {
    pub(crate) fn new(
        sync_config: SyncConfig,
        node_client: NodeClient,
        context_client: ContextClient,
        node_state: crate::NodeState,
    ) -> Self {
        Self {
            sync_config,
            node_client,
            context_client,
            node_state,
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

        let (context_id, their_identity, payload, nonce) = match message {
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

        let mut _updated = None;

        if !self
            .context_client
            .has_member(&context_id, &their_identity)?
        {
            _updated = Some(
                self.context_client
                    .sync_context_config(context_id, None)
                    .await?,
            );

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

        let Some((our_identity, _)) =
            crate::utils::choose_stream(identities, &mut rand::thread_rng())
                .await
                .transpose()?
        else {
            bail!("no owned identities found for context: {}", context.id);
        };

        match payload {
            InitPayload::KeyShare => {
                self.handle_key_share_request(&context, our_identity, their_identity, stream, nonce)
                    .await?
            }
            InitPayload::BlobShare { blob_id } => {
                self.handle_blob_share_request(
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
                self.handle_delta_request(requested_context_id, delta_id, stream)
                    .await?
            }
            InitPayload::DagHeadsRequest {
                context_id: requested_context_id,
            } => {
                self.handle_dag_heads_request(requested_context_id, stream)
                    .await?
            }
        };

        Ok(Some(()))
    }

    async fn handle_key_share_request(
        &self,
        context: &calimero_context_primitives::types::Context,
        our_identity: calimero_primitives::identity::PublicKey,
        their_identity: calimero_primitives::identity::PublicKey,
        stream: &mut Stream,
        nonce: Nonce,
    ) -> Result<()> {
        super::key::handle_key_share_request(
            &self.node_client,
            &self.context_client,
            context,
            our_identity,
            their_identity,
            stream,
            nonce,
        )
        .await
    }

    async fn handle_blob_share_request(
        &self,
        context: &calimero_context_primitives::types::Context,
        our_identity: calimero_primitives::identity::PublicKey,
        their_identity: calimero_primitives::identity::PublicKey,
        blob_id: calimero_primitives::blobs::BlobId,
        stream: &mut Stream,
    ) -> Result<()> {
        super::blobs::handle_blob_share_request(
            &self.node_client,
            &self.context_client,
            context,
            our_identity,
            their_identity,
            blob_id,
            stream,
        )
        .await
    }

    async fn handle_delta_request(
        &self,
        context_id: ContextId,
        delta_id: [u8; 32],
        stream: &mut Stream,
    ) -> Result<()> {
        super::delta_request::handle_delta_request(
            &self.context_client,
            &self.node_state,
            context_id,
            delta_id,
            stream,
        )
        .await
    }

    async fn handle_dag_heads_request(
        &self,
        context_id: ContextId,
        stream: &mut Stream,
    ) -> Result<()> {
        super::delta_request::handle_dag_heads_request(&self.context_client, context_id, stream)
            .await
    }
}
