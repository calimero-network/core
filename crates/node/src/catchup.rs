use calimero_network::stream::{Message, Stream};
use calimero_primitives::context::{Context, ContextId};
use eyre::{bail, Result as EyreResult};
use futures_util::{SinkExt, StreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::thread_rng;
use serde_json::{from_slice as from_json_slice, to_vec as to_json_vec};
use tokio::time::timeout;
use tracing::{error, info, warn};

use crate::types::{ActionMessage, CatchupError, CatchupStreamMessage, CatchupSyncRequest};
use crate::Node;

mod batch;

impl Node {
    pub(crate) async fn handle_opened_stream(&self, mut stream: Box<Stream>) -> EyreResult<()> {
        let Some(message) = stream.next().await else {
            bail!("Stream closed unexpectedly")
        };

        let request = match from_json_slice(&message?.data)? {
            CatchupStreamMessage::SyncRequest(req) => req,
            message @ (CatchupStreamMessage::ActionsBatch(_)
            | CatchupStreamMessage::ApplicationBlobRequest(_)
            | CatchupStreamMessage::ApplicationBlobChunk(_)
            | CatchupStreamMessage::Error(_)) => {
                bail!("Unexpected message: {:?}", message)
            }
        };

        let Some(context) = self.ctx_manager.get_context(&request.context_id)? else {
            let message = to_json_vec(&CatchupStreamMessage::Error(
                CatchupError::ContextNotFound {
                    context_id: request.context_id,
                },
            ))?;
            stream.send(Message::new(message)).await?;

            return Ok(());
        };

        info!(
            request=?request,
            root_hash=%context.root_hash,
            "Processing catchup request for context",
        );

        let _handle = self.store.handle();

        // TODO: If the root hashes don't match, we need to start a comparison
        if context.root_hash != request.root_hash {
            bail!("Root hash mismatch: TODO");
        }

        Ok(())
    }

    pub(crate) async fn handle_interval_catchup(&mut self) {
        let Some(context_id) = self.ctx_manager.get_any_pending_catchup_context().await else {
            return;
        };

        let peers = self
            .network_client
            .mesh_peers(TopicHash::from_raw(context_id))
            .await;
        let Some(peer_id) = peers.choose(&mut thread_rng()) else {
            return;
        };

        info!(%context_id, %peer_id, "Attempting to perform interval triggered catchup");

        if let Err(err) = self.perform_catchup(context_id, *peer_id).await {
            error!(%err, "Failed to perform interval catchup");
            return;
        }

        let _ = self
            .ctx_manager
            .clear_context_pending_catchup(&context_id)
            .await;

        info!(%context_id, %peer_id, "Interval triggered catchup successfully finished");
    }

    // TODO: Is this even needed now? Can it be removed? In theory, a sync will
    // TODO: take place so long as there is a comparison - i.e. it will send
    // TODO: everything back and forth until everything matches. But, for e.g. a
    // TODO: first-time sync, that would be slower than just sending everything
    // TODO: all at once. So... could this be utilised for that?
    pub(crate) async fn perform_catchup(
        &mut self,
        context_id: ContextId,
        chosen_peer: PeerId,
    ) -> EyreResult<()> {
        let Some(mut context) = self.ctx_manager.get_context(&context_id)? else {
            bail!("catching up for non-existent context?");
        };

        let request = CatchupSyncRequest {
            context_id,
            root_hash: context.root_hash,
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        let data = to_json_vec(&CatchupStreamMessage::SyncRequest(request))?;

        stream.send(Message::new(data)).await?;

        // todo! ask peer for the application if we don't have it

        loop {
            let message = timeout(
                self.network_client.catchup_config.receive_timeout,
                stream.next(),
            )
            .await;

            match message {
                Ok(message) => match message {
                    Some(message) => {
                        self.handle_catchup_message(
                            chosen_peer,
                            &mut context,
                            from_json_slice(&message?.data)?,
                        )
                        .await?;
                    }
                    None => break,
                },
                Err(err) => {
                    bail!("Timeout while waiting for catchup message: {}", err);
                }
            }
        }

        Ok(())
    }

    async fn handle_catchup_message(
        &mut self,
        // TODO: How should this be used?
        _chosen_peer: PeerId,
        context: &mut Context,
        message: CatchupStreamMessage,
    ) -> EyreResult<()> {
        match message {
            CatchupStreamMessage::ActionsBatch(batch) => {
                info!(
                    context_id=%context.id,
                    actions=%batch.actions.len(),
                    "Processing catchup actions batch"
                );

                for ActionMessage {
                    actions,
                    public_key,
                    ..
                } in batch.actions
                {
                    for action in actions {
                        self.apply_action(context, &action, public_key).await?;
                    }
                }
            }
            CatchupStreamMessage::Error(err) => {
                error!(?err, "Received error during catchup");
                bail!(err);
            }
            CatchupStreamMessage::SyncRequest(request) => {
                warn!("Unexpected message: {:?}", request);
            }
            CatchupStreamMessage::ApplicationBlobRequest(_)
            | CatchupStreamMessage::ApplicationBlobChunk(_) => {
                bail!("Unexpected message");
            }
        }

        Ok(())
    }
}
