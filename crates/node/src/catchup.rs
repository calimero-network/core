use std::collections::VecDeque;
use std::io::{Error as StdIoError, ErrorKind as StdIoErrorKind};

use calimero_network::stream::{Message, Stream};
use calimero_node_primitives::NodeType;
use calimero_primitives::application::Application;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::transaction::Transaction;
use calimero_store::key::ContextTransaction as ContextTransactionKey;
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
use crate::catchup::transactions::TransactionsBatchSender;
use crate::transaction_pool::TransactionPoolEntry;
use crate::types::{
    CatchupApplicationBlobChunk, CatchupApplicationBlobRequest, CatchupError, CatchupStreamMessage,
    CatchupTransactionsBatch, CatchupTransactionsRequest, TransactionStatus, TransactionWithStatus,
};
use crate::Node;

mod blobs;
mod transactions;

impl Node {
    pub(crate) async fn handle_opened_stream(&self, mut stream: Box<Stream>) -> EyreResult<()> {
        let Some(message) = stream.next().await else {
            bail!("Stream closed unexpectedly")
        };

        match from_json_slice(&message?.data)? {
            CatchupStreamMessage::TransactionsRequest(req) => {
                self.handle_transaction_catchup(req, stream).await
            }
            CatchupStreamMessage::ApplicationBlobRequest(req) => {
                self.handle_blob_catchup(req, stream).await
            }
            message @ (CatchupStreamMessage::TransactionsBatch(_)
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

        let mut blob_sender = ApplicationBlobChunkSender::new(stream)?;

        while let Some(chunk) = blob.try_next().await? {
            blob_sender.send(&chunk).await?;
        }

        blob_sender.flush().await?;

        todo!()
    }

    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    async fn handle_transaction_catchup(
        &self,
        request: CatchupTransactionsRequest,
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
            last_transaction_hash=%context.last_transaction_hash,
            "Processing catchup request for context",
        );

        let handle = self.store.handle();

        if request.last_executed_transaction_hash != Hash::default()
            && !handle.has(&ContextTransactionKey::new(
                request.context_id,
                request.last_executed_transaction_hash.into(),
            ))?
        {
            let message = to_json_vec(&CatchupStreamMessage::Error(
                CatchupError::TransactionNotFound {
                    transaction_hash: request.last_executed_transaction_hash,
                },
            ))?;
            stream.send(Message::new(message)).await?;

            return Ok(());
        };

        if context.last_transaction_hash == request.last_executed_transaction_hash
            && self.tx_pool.is_empty()
        {
            return Ok(());
        }

        let mut hashes = VecDeque::new();

        let mut last_transaction_hash = context.last_transaction_hash;

        while last_transaction_hash != Hash::default()
            && last_transaction_hash != request.last_executed_transaction_hash
        {
            let key = ContextTransactionKey::new(request.context_id, last_transaction_hash.into());

            let Some(transaction) = handle.get(&key)? else {
                error!(
                    context_id=%request.context_id,
                    %last_transaction_hash,
                    "Context transaction not found, our transaction chain might be corrupted"
                );

                let message =
                    to_json_vec(&CatchupStreamMessage::Error(CatchupError::InternalError))?;
                stream.send(Message::new(message)).await?;

                return Ok(());
            };

            hashes.push_front(last_transaction_hash);

            last_transaction_hash = transaction.prior_hash.into();
        }

        let mut batch_writer = TransactionsBatchSender::new(request.batch_size, stream);

        for hash in hashes {
            let key = ContextTransactionKey::new(request.context_id, hash.into());
            let Some(transaction) = handle.get(&key)? else {
                error!(
                    context_id=%request.context_id,
                    ?hash,
                    "Context transaction not found after the initial check. This is most likely a BUG!"
                );
                batch_writer
                    .flush_with_error(CatchupError::InternalError)
                    .await?;
                return Ok(());
            };

            batch_writer
                .send(TransactionWithStatus {
                    transaction_hash: hash,
                    transaction: Transaction::new(
                        request.context_id,
                        transaction.method.into(),
                        transaction.payload.into(),
                        Hash::from(transaction.prior_hash),
                        transaction.executor_public_key.into(),
                    ),
                    status: TransactionStatus::Executed,
                })
                .await?;
        }

        for (hash, TransactionPoolEntry { transaction, .. }) in self.tx_pool.iter() {
            batch_writer
                .send(TransactionWithStatus {
                    transaction_hash: *hash,
                    transaction: Transaction::new(
                        request.context_id,
                        transaction.method.clone(),
                        transaction.payload.clone(),
                        transaction.prior_hash,
                        transaction.executor_public_key,
                    ),
                    status: TransactionStatus::Pending,
                })
                .await?;
        }

        batch_writer.flush().await?;

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

        self.perform_transaction_catchup(chosen_peer, &mut context)
            .await
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

        let data = to_json_vec(&request)?;

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
                        message @ (CatchupStreamMessage::TransactionsBatch(_)
                        | CatchupStreamMessage::TransactionsRequest(_)
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

    async fn perform_transaction_catchup(
        &mut self,
        chosen_peer: PeerId,
        context: &mut Context,
    ) -> EyreResult<()> {
        let request = CatchupTransactionsRequest {
            context_id: context.id,
            last_executed_transaction_hash: context.last_transaction_hash,
            batch_size: self.network_client.catchup_config.batch_size,
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        let data = to_json_vec(&CatchupStreamMessage::TransactionsRequest(request))?;

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
                        CatchupStreamMessage::TransactionsBatch(batch) => {
                            self.apply_transactions_batch(chosen_peer, context, batch)
                                .await?;
                        }
                        message @ (CatchupStreamMessage::ApplicationBlobChunk(_)
                        | CatchupStreamMessage::TransactionsRequest(_)
                        | CatchupStreamMessage::ApplicationBlobRequest(_)) => {
                            warn!("Ignoring unexpected message: {:?}", message);
                        }
                        CatchupStreamMessage::Error(err) => {
                            error!(?err, "Received error during transaction catchup");
                            bail!(err);
                        }
                    },
                    None => break,
                },
                Err(err) => bail!("Failed to await transactions catchup message: {}", err),
            }
        }

        Ok(())
    }

    async fn apply_transactions_batch(
        &mut self,
        chosen_peer: PeerId,
        context: &mut Context,
        batch: CatchupTransactionsBatch,
    ) -> EyreResult<()> {
        info!(
            context_id=%context.id,
            transactions=%batch.transactions.len(),
            "Processing catchup transactions batch"
        );

        for TransactionWithStatus {
            transaction_hash,
            transaction,
            status,
        } in batch.transactions
        {
            if context.last_transaction_hash != transaction.prior_hash {
                bail!(
                        "Transaction '{}' from the catchup batch doesn't build on last transaction '{}'",
                        transaction_hash,
                        context.last_transaction_hash,
                    );
            };

            match status {
                TransactionStatus::Pending => match self.typ {
                    NodeType::Peer => {
                        let _ = self.tx_pool.insert(
                            chosen_peer,
                            Transaction::new(
                                context.id,
                                transaction.method,
                                transaction.payload,
                                transaction.prior_hash,
                                transaction.executor_public_key,
                            ),
                            None,
                        )?;
                    }
                    NodeType::Coordinator => {
                        let _ = self
                            .validate_pending_transaction(context, transaction, transaction_hash)
                            .await?;

                        drop(self.tx_pool.remove(&transaction_hash));
                    }
                    _ => bail!("Unexpected node type"),
                },
                TransactionStatus::Executed => match self.typ {
                    NodeType::Peer => {
                        drop(
                            self.execute_transaction(context, transaction, transaction_hash)
                                .await?,
                        );

                        drop(self.tx_pool.remove(&transaction_hash));
                    }
                    NodeType::Coordinator => {
                        self.persist_transaction(context, transaction, transaction_hash)?;
                    }
                    _ => bail!("Unexpected node type"),
                },
            }

            context.last_transaction_hash = transaction_hash;
        }

        Ok(())
    }
}
