use std::io::{Error as StdIoError, ErrorKind as StdIoErrorKind};

use calimero_network::stream::{Message, Stream};
use calimero_primitives::application::Application;
use calimero_primitives::context::{Context, ContextId};
use eyre::{bail, Result as EyreResult};
use futures_util::io::BufReader;
use futures_util::stream::poll_fn;
use futures_util::{SinkExt, StreamExt, TryStreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::thread_rng;
use serde_json::{from_slice as from_json_slice, to_vec as to_json_vec};
use tokio::spawn;
use tokio::sync::mpsc;
use tokio::time::timeout;
use tracing::{error, info, warn};
use url::Url;

use crate::catchup::blobs::ApplicationBlobChunkSender;
use crate::types::{
    ActionMessage, CatchupActionsBatch, CatchupApplicationBlobChunk, CatchupApplicationBlobRequest,
    CatchupError, CatchupStreamMessage, CatchupSyncRequest,
};
use crate::Node;

mod actions;
mod blobs;

impl Node {
    pub(crate) async fn handle_opened_stream(&self, mut stream: Box<Stream>) -> EyreResult<()> {
        let Some(message) = stream.next().await else {
            bail!("Stream closed unexpectedly")
        };

        match from_json_slice(&message?.data)? {
            CatchupStreamMessage::SyncRequest(req) => self.handle_action_catchup(req, stream).await,
            CatchupStreamMessage::ApplicationBlobRequest(req) => {
                self.handle_blob_catchup(req, stream).await
            }
            message @ (CatchupStreamMessage::ActionsBatch(_)
            | CatchupStreamMessage::ApplicationBlobChunk(_)
            | CatchupStreamMessage::Error(_)) => {
                bail!("Unexpected message: {:?}", message)
            }
        }
    }

    async fn handle_blob_catchup(
        &self,
        request: CatchupApplicationBlobRequest,
        mut stream: Box<Stream>,
    ) -> Result<(), eyre::Error> {
        let Some(mut blob) = self
            .ctx_manager
            .get_application_blob(&request.application_id)?
        else {
            let message = to_json_vec(&CatchupStreamMessage::Error(
                CatchupError::ApplicationNotFound {
                    application_id: request.application_id,
                },
            ))?;
            stream.send(Message::new(message)).await?;

            return Ok(());
        };

        info!(
            request=?request,
            "Processing application blob catchup request",
        );

        let mut blob_sender = ApplicationBlobChunkSender::new(stream);

        while let Some(chunk) = blob.try_next().await? {
            blob_sender.send(&chunk).await?;
        }

        blob_sender.flush().await
    }

    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    async fn handle_action_catchup(
        &self,
        request: CatchupSyncRequest,
        mut stream: Box<Stream>,
    ) -> EyreResult<()> {
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
            "Processing context catchup request",
        );

        let _handle = self.store.handle();

        // TODO: If the root hashes don't match, we need to start a comparison
        if context.root_hash != request.root_hash {
            bail!("Root hash mismatch: TODO");
        }

        Ok(())
    }

    pub(crate) async fn perform_interval_catchup(&mut self) {
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

        let latest_application = self.ctx_manager.get_latest_application(context_id).await?;
        let local_application = self.ctx_manager.get_application(&latest_application.id)?;

        if local_application.map_or(true, |app| app.blob != latest_application.blob)
            || !self
                .ctx_manager
                .is_application_blob_installed(latest_application.blob)?
        {
            self.perform_blob_catchup(chosen_peer, latest_application)
                .await?;
        }

        self.perform_action_catchup(chosen_peer, &mut context).await
    }

    async fn perform_blob_catchup(
        &mut self,
        chosen_peer: PeerId,
        latest_application: Application,
    ) -> EyreResult<()> {
        let source = Url::from(latest_application.source.clone());

        match source.scheme() {
            "http" | "https" => {
                info!("Skipping blob catchup for HTTP/HTTPS source");
                return Ok(());
            }
            _ => {
                self.perform_blob_stream_catchup(chosen_peer, latest_application)
                    .await
            }
        }
    }

    async fn perform_blob_stream_catchup(
        &mut self,
        chosen_peer: PeerId,
        latest_application: Application,
    ) -> EyreResult<()> {
        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        let request = CatchupApplicationBlobRequest {
            application_id: latest_application.id,
        };

        let data = to_json_vec(&CatchupStreamMessage::ApplicationBlobRequest(request))?;

        stream.send(Message::new(data)).await?;

        let (tx, mut rx) = mpsc::channel(100);
        let mut current_sequential_id = 0;

        let chunk_stream = BufReader::new(
            poll_fn(move |cx| rx.poll_recv(cx))
                .map(move |chunk: CatchupApplicationBlobChunk| {
                    if chunk.sequential_id != current_sequential_id {
                        return Err(StdIoError::new(
                            StdIoErrorKind::InvalidData,
                            format!(
                                "invalid sequential id, expected: {expected}, got: {got}",
                                expected = current_sequential_id,
                                got = chunk.sequential_id
                            ),
                        ));
                    }

                    current_sequential_id += 1;

                    Ok(chunk.chunk)
                })
                .into_async_read(),
        );

        let ctx_manager = self.ctx_manager.clone();
        let metadata = latest_application.metadata.clone();

        let handle = spawn(async move {
            ctx_manager
                .install_application_from_stream(
                    latest_application.size,
                    chunk_stream,
                    &latest_application.source,
                    metadata,
                )
                .await
                .map(|_| ())
        });

        loop {
            match timeout(
                self.network_client.catchup_config.receive_timeout,
                stream.next(),
            )
            .await
            {
                Ok(message) => match message {
                    Some(message) => match from_json_slice(&message?.data)? {
                        CatchupStreamMessage::ApplicationBlobChunk(chunk) => {
                            tx.send(chunk).await?;
                        }
                        message @ (CatchupStreamMessage::ActionsBatch(_)
                        | CatchupStreamMessage::SyncRequest(_)
                        | CatchupStreamMessage::ApplicationBlobRequest(_)) => {
                            warn!("Ignoring unexpected message: {:?}", message);
                        }
                        CatchupStreamMessage::Error(err) => {
                            error!(?err, "Received error during application blob catchup");
                            bail!(err);
                        }
                    },
                    None => break,
                },
                Err(err) => {
                    bail!("Failed to await application blob chunk message: {}", err)
                }
            }
        }

        drop(tx);

        handle.await?
    }

    async fn perform_action_catchup(
        &mut self,
        chosen_peer: PeerId,
        context: &mut Context,
    ) -> EyreResult<()> {
        let request = CatchupSyncRequest {
            context_id: context.id,
            root_hash: context.root_hash,
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        let data = to_json_vec(&CatchupStreamMessage::SyncRequest(request))?;

        stream.send(Message::new(data)).await?;

        loop {
            match timeout(
                self.network_client.catchup_config.receive_timeout,
                stream.next(),
            )
            .await
            {
                Ok(message) => match message {
                    Some(message) => match from_json_slice(&message?.data)? {
                        CatchupStreamMessage::ActionsBatch(batch) => {
                            self.apply_actions_batch(chosen_peer, context, batch)
                                .await?;
                        }
                        message @ (CatchupStreamMessage::ApplicationBlobChunk(_)
                        | CatchupStreamMessage::SyncRequest(_)
                        | CatchupStreamMessage::ApplicationBlobRequest(_)) => {
                            warn!("Ignoring unexpected message: {:?}", message);
                        }
                        CatchupStreamMessage::Error(err) => {
                            error!(?err, "Received error during action catchup");
                            bail!(err);
                        }
                    },
                    None => break,
                },
                Err(err) => bail!("Failed to await actions catchup message: {}", err),
            }
        }

        Ok(())
    }

    async fn apply_actions_batch(
        &mut self,
        // TODO: How should this be used?
        _chosen_peer: PeerId,
        context: &mut Context,
        batch: CatchupActionsBatch,
    ) -> EyreResult<()> {
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

        Ok(())
    }
}
