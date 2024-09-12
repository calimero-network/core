use std::collections::VecDeque;

use calimero_network::stream::{Message, Stream};
use calimero_node_primitives::NodeType;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::transaction::Transaction;
use calimero_store::key::ContextTransaction as ContextTransactionKey;
use eyre::{bail, Result as EyreResult};
use futures_util::{SinkExt, StreamExt};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;
use rand::seq::SliceRandom;
use rand::thread_rng;
use serde_json::{from_slice as from_json_slice, to_vec as to_json_vec};
use tokio::time::timeout;
use tracing::{error, info, warn};

use crate::catchup::batch::CatchupBatchSender;
use crate::transaction_pool::TransactionPoolEntry;
use crate::types::{
    CatchupError, CatchupRequest, CatchupStreamMessage, TransactionStatus, TransactionWithStatus,
};
use crate::Node;

mod batch;

#[allow(clippy::multiple_inherent_impl)]
impl Node {
    // TODO: Consider splitting this long function into multiple parts.
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn handle_opened_stream(&self, mut stream: Box<Stream>) -> EyreResult<()> {
        let Some(message) = stream.next().await else {
            bail!("Stream closed unexpectedly")
        };

        let request = match from_json_slice(&message?.data)? {
            CatchupStreamMessage::Request(req) => req,
            message @ (CatchupStreamMessage::TransactionsBatch(_)
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

        let mut batch_writer = CatchupBatchSender::new(request.batch_size, stream);

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

    pub(crate) async fn perform_catchup(
        &mut self,
        context_id: ContextId,
        chosen_peer: PeerId,
    ) -> EyreResult<()> {
        let Some(mut context) = self.ctx_manager.get_context(&context_id)? else {
            bail!("catching up for non-existent context?");
        };

        let request = CatchupRequest {
            context_id,
            last_executed_transaction_hash: context.last_transaction_hash,
            batch_size: self.network_client.catchup_config.batch_size,
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        let data = to_json_vec(&CatchupStreamMessage::Request(request))?;

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

    // TODO: Consider splitting this long function into multiple parts.
    #[allow(clippy::too_many_lines)]
    async fn handle_catchup_message(
        &mut self,
        chosen_peer: PeerId,
        context: &mut Context,
        message: CatchupStreamMessage,
    ) -> EyreResult<()> {
        match message {
            CatchupStreamMessage::TransactionsBatch(batch) => {
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
                                    .validate_pending_transaction(
                                        context,
                                        transaction,
                                        transaction_hash,
                                    )
                                    .await?;

                                drop(self.tx_pool.remove(&transaction_hash));
                            }
                            _ => bail!("Unexpected node type"),
                        },
                        TransactionStatus::Executed => match self.typ {
                            NodeType::Peer => {
                                drop(
                                    self.execute_transaction(
                                        context,
                                        transaction,
                                        transaction_hash,
                                    )
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
            }
            CatchupStreamMessage::Error(err) => {
                error!(?err, "Received error during catchup");
                bail!(err);
            }
            CatchupStreamMessage::Request(request) => {
                warn!("Unexpected message: {:?}", request);
            }
        }

        Ok(())
    }
}
