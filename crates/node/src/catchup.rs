use std::collections::VecDeque;

use futures_util::{SinkExt, StreamExt};
use libp2p::gossipsub::TopicHash;
use rand::seq::SliceRandom;
use tracing::{error, info, warn};

use crate::transaction_pool::TransactionPoolEntry;
use crate::{types, Node};

mod batch;

impl Node {
    pub(crate) async fn handle_opened_stream(
        &mut self,
        mut stream: Box<calimero_network::stream::Stream>,
    ) -> eyre::Result<()> {
        let Some(message) = stream.next().await else {
            eyre::bail!("Stream closed unexpectedly")
        };

        let request = match serde_json::from_slice(&message?.data)? {
            types::CatchupStreamMessage::Request(req) => req,
            message => {
                eyre::bail!("Unexpected message: {:?}", message)
            }
        };

        let Some(context) = self.ctx_manager.get_context(&request.context_id)? else {
            let message = serde_json::to_vec(&types::CatchupStreamMessage::Error(
                types::CatchupError::ContextNotFound {
                    context_id: request.context_id,
                },
            ))?;
            stream
                .send(calimero_network::stream::Message { data: message })
                .await?;

            return Ok(());
        };

        info!(
            request=?request,
            last_transaction_hash=%context.last_transaction_hash,
            "Processing catchup request for context",
        );

        let handle = self.store.handle();

        if request.last_executed_transaction_hash != calimero_primitives::hash::Hash::default()
            && !handle.has(&calimero_store::key::ContextTransaction::new(
                request.context_id,
                request.last_executed_transaction_hash.into(),
            ))?
        {
            let message = serde_json::to_vec(&types::CatchupStreamMessage::Error(
                types::CatchupError::TransactionNotFound {
                    transaction_hash: request.last_executed_transaction_hash,
                },
            ))?;
            stream
                .send(calimero_network::stream::Message { data: message })
                .await?;

            return Ok(());
        };

        let application_id = context.application_id.clone();

        if request
            .application_id
            .map_or(true, |id| id != application_id)
        {
            let Some(application) = self.ctx_manager.get_application(&application_id)? else {
                eyre::bail!(
                    "fatal error: context `{}` links to dangling application ID `{}`",
                    context.id,
                    application_id
                );
            };

            let message = serde_json::to_vec(&types::CatchupStreamMessage::ApplicationChanged(
                types::CatchupApplicationChanged {
                    application_id,
                    blob_id: application.blob,
                    version: application.version,
                    source: application.source,
                    hash: None, // todo! blob_mgr(application.blob)?.hash
                    metadata: Some(Vec::new()),
                },
            ))?;
            stream
                .send(calimero_network::stream::Message { data: message })
                .await?;
        }

        if context.last_transaction_hash == request.last_executed_transaction_hash
            && self.tx_pool.is_empty()
        {
            return Ok(());
        }

        let mut hashes = VecDeque::new();

        let mut last_transaction_hash = context.last_transaction_hash;

        while last_transaction_hash != calimero_primitives::hash::Hash::default()
            && last_transaction_hash != request.last_executed_transaction_hash
        {
            let key = calimero_store::key::ContextTransaction::new(
                request.context_id,
                last_transaction_hash.into(),
            );

            let Some(transaction) = handle.get(&key)? else {
                error!(
                    context_id=%request.context_id,
                    %last_transaction_hash,
                    "Context transaction not found, our transaction chain might be corrupted"
                );

                let message = serde_json::to_vec(&types::CatchupStreamMessage::Error(
                    types::CatchupError::InternalError,
                ))?;
                stream
                    .send(calimero_network::stream::Message { data: message })
                    .await?;

                return Ok(());
            };

            hashes.push_front(last_transaction_hash);

            last_transaction_hash = transaction.prior_hash.into();
        }

        let mut batch_writer = batch::CatchupBatchSender::new(request.batch_size, stream);

        for hash in hashes {
            let key = calimero_store::key::ContextTransaction::new(request.context_id, hash.into());
            let Some(transaction) = handle.get(&key)? else {
                error!(
                    context_id=%request.context_id,
                    ?hash,
                    "Context transaction not found after the initial check. This is most likely a BUG!"
                );
                batch_writer
                    .flush_with_error(types::CatchupError::InternalError)
                    .await?;
                return Ok(());
            };

            batch_writer
                .send(types::TransactionWithStatus {
                    transaction_hash: hash,
                    transaction: calimero_primitives::transaction::Transaction {
                        context_id: request.context_id,
                        method: transaction.method.into(),
                        payload: transaction.payload.into(),
                        prior_hash: calimero_primitives::hash::Hash::from(transaction.prior_hash),
                        executor_public_key: transaction.executor_public_key,
                    },
                    status: types::TransactionStatus::Executed,
                })
                .await?;
        }

        for (hash, TransactionPoolEntry { transaction, .. }) in self.tx_pool.iter() {
            batch_writer
                .send(types::TransactionWithStatus {
                    transaction_hash: *hash,
                    transaction: calimero_primitives::transaction::Transaction {
                        context_id: request.context_id,
                        method: transaction.method.clone(),
                        payload: transaction.payload.clone(),
                        prior_hash: transaction.prior_hash,
                        executor_public_key: transaction.executor_public_key,
                    },
                    status: types::TransactionStatus::Pending,
                })
                .await?;
        }

        batch_writer.flush().await?;

        Ok(())
    }

    pub(crate) async fn handle_interval_catchup(&mut self) {
        let context_id = match self.ctx_manager.get_any_pending_catchup_context().await {
            Some(context_id) => context_id.clone(),
            None => return,
        };

        let peer_id = match self
            .network_client
            .mesh_peers(TopicHash::from_raw(context_id))
            .await
            .choose(&mut rand::thread_rng())
        {
            Some(peer_id) => peer_id.clone(),
            None => return,
        };

        info!(%context_id, %peer_id, "Attempting to perform interval triggered catchup");

        if let Err(err) = self.perform_catchup(context_id, peer_id).await {
            error!(%err, "Failed to perform interval catchup");
            return;
        }

        self.ctx_manager
            .clear_context_pending_catchup(&context_id)
            .await;

        info!(%context_id, %peer_id, "Interval triggered catchup successfully finished");
    }

    pub(crate) async fn perform_catchup(
        &mut self,
        context_id: calimero_primitives::context::ContextId,
        chosen_peer: libp2p::PeerId,
    ) -> eyre::Result<()> {
        let (mut context, request) = match self.ctx_manager.get_context(&context_id)? {
            Some(context) => (
                Some(context.clone()),
                types::CatchupRequest {
                    context_id,
                    application_id: Some(context.application_id),
                    last_executed_transaction_hash: context.last_transaction_hash,
                    batch_size: self.network_client.catchup_config.batch_size,
                },
            ),
            None => (
                None,
                types::CatchupRequest {
                    context_id,
                    application_id: None,
                    last_executed_transaction_hash: calimero_primitives::hash::Hash::default(),
                    batch_size: self.network_client.catchup_config.batch_size,
                },
            ),
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        let data = serde_json::to_vec(&types::CatchupStreamMessage::Request(request))?;

        stream
            .send(calimero_network::stream::Message { data })
            .await?;

        loop {
            let message = tokio::time::timeout(
                self.network_client.catchup_config.receive_timeout,
                stream.next(),
            )
            .await;

            match message {
                Ok(message) => match message {
                    Some(message) => {
                        context = self
                            .handle_catchup_message(
                                context_id,
                                chosen_peer,
                                context,
                                serde_json::from_slice(&message?.data)?,
                            )
                            .await?;
                    }
                    None => break,
                },
                Err(err) => {
                    eyre::bail!("Timeout while waiting for catchup message: {}", err);
                }
            }
        }

        Ok(())
    }

    async fn handle_catchup_message(
        &mut self,
        context_id: calimero_primitives::context::ContextId,
        chosen_peer: libp2p::PeerId,
        mut context: Option<calimero_primitives::context::Context>,
        message: types::CatchupStreamMessage,
    ) -> eyre::Result<Option<calimero_primitives::context::Context>> {
        match message {
            types::CatchupStreamMessage::TransactionsBatch(batch) => {
                let Some(ref mut context_) = context else {
                    eyre::bail!("Received transactions batch for uninitialized context");
                };

                info!(
                    context_id=%context_.id,
                    transactions=%batch.transactions.len(),
                    "Processing catchup transactions batch"
                );

                for types::TransactionWithStatus {
                    transaction_hash,
                    transaction,
                    status,
                } in batch.transactions
                {
                    if context_.last_transaction_hash != transaction.prior_hash {
                        eyre::bail!(
                            "Transaction '{}' from the catchup batch doesn't build on last transaction '{}'",
                            transaction_hash,
                            context_.last_transaction_hash,
                        );
                    };

                    match status {
                        types::TransactionStatus::Pending => match self.typ {
                            calimero_node_primitives::NodeType::Peer => {
                                self.tx_pool.insert(
                                    chosen_peer,
                                    calimero_primitives::transaction::Transaction {
                                        context_id: context_.id,
                                        method: transaction.method,
                                        payload: transaction.payload,
                                        prior_hash: transaction.prior_hash,
                                        executor_public_key: transaction.executor_public_key,
                                    },
                                    None,
                                )?;
                            }
                            calimero_node_primitives::NodeType::Coordinator => {
                                self.validate_pending_transaction(
                                    context_.clone(),
                                    transaction,
                                    transaction_hash,
                                )
                                .await?;

                                self.tx_pool.remove(&transaction_hash);
                            }
                        },
                        types::TransactionStatus::Executed => match self.typ {
                            calimero_node_primitives::NodeType::Peer => {
                                self.execute_transaction(
                                    context_.clone(),
                                    transaction.clone(),
                                    transaction_hash,
                                )
                                .await?;

                                self.tx_pool.remove(&transaction_hash);
                            }
                            calimero_node_primitives::NodeType::Coordinator => {
                                self.persist_transaction(
                                    context_.clone(),
                                    transaction.clone(),
                                    transaction_hash,
                                )?;
                            }
                        },
                    }

                    context_.last_transaction_hash = transaction_hash;
                }
            }
            types::CatchupStreamMessage::ApplicationChanged(change) => {
                info!(?change, "Processing catchup application changed");

                if !self
                    .ctx_manager
                    .is_application_installed(&change.application_id)?
                {
                    if change.source.to_string().starts_with("http://")
                        || change.source.to_string().starts_with("https://")
                    {
                        info!("Installing application from the url");
                        self.ctx_manager
                            .install_application_from_url(
                                change.source.to_string().parse()?,
                                change.version,
                                Vec::new(),
                            )
                            .await?;
                    } else {
                        // TODO: for path sources, share the blob peer to peer
                        // NOTE: this will fail if the path is not accessible by the node
                        info!("Installing application from the path");

                        self.ctx_manager
                            .install_application_from_path(
                                change.source.to_string().parse()?,
                                change.version,
                                Vec::new(),
                            )
                            .await?;
                    }
                }

                match context {
                    Some(ref mut context_) => {
                        self.ctx_manager
                            .update_application_id(context_.id, change.application_id.clone())?;

                        context_.application_id = change.application_id;
                    }
                    None => {
                        let context_inner = calimero_primitives::context::Context {
                            id: context_id,
                            application_id: change.application_id,
                            last_transaction_hash: calimero_primitives::hash::Hash::default(),
                        };

                        self.ctx_manager.add_context(&context_inner, None).await?;

                        context = Some(context_inner);
                    }
                }
            }
            types::CatchupStreamMessage::Error(err) => {
                error!(?err, "Received error during catchup");
                eyre::bail!(err);
            }
            types::CatchupStreamMessage::Request(request) => {
                warn!("Unexpected message: {:?}", request)
            }
        }

        Ok(context)
    }
}
