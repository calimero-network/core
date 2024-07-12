use std::collections::VecDeque;

use futures_util::{SinkExt, StreamExt};
use tracing::{error, warn};

use crate::{types, Node};

mod batch;

impl Node {
    pub(crate) async fn handle_opened_stream(
        &mut self,
        mut stream: calimero_network::stream::Stream,
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

        if request.application_id.is_none() || application_id == request.application_id.unwrap() {
            let application_version = self
                .ctx_manager
                .get_application_latest_version(&application_id)?;

            let message = serde_json::to_vec(&types::CatchupStreamMessage::ApplicationChanged(
                types::CatchupApplicationChanged {
                    application_id,
                    version: application_version,
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

        let mut current_hash = context.last_transaction_hash;

        while current_hash != calimero_primitives::hash::Hash::default()
            && current_hash != request.last_executed_transaction_hash
        {
            let key = calimero_store::key::ContextTransaction::new(
                request.context_id,
                current_hash.into(),
            );

            let Some(transaction) = handle.get(&key)? else {
                error!(
                    context_id=%request.context_id,
                    ?current_hash,
                    "Context transaction not found, our transaction chain might be corrupted!!!"
                );

                let message = serde_json::to_vec(&types::CatchupStreamMessage::Error(
                    types::CatchupError::InternalError,
                ))?;
                stream
                    .send(calimero_network::stream::Message { data: message })
                    .await?;

                return Ok(());
            };

            hashes.push_front(transaction.prior_hash);

            current_hash = transaction.prior_hash.into();
        }

        let mut batch_writer = batch::CatchupBatchSender::new(request.batch_size, stream);

        for hash in hashes {
            let key = calimero_store::key::ContextTransaction::new(request.context_id, hash.into());
            let Some(transaction) = handle.get(&key)? else {
                error!(
                    context_id=%request.context_id,
                    ?hash,
                    "Context transaction not found, our transaction chain might be corrupted!!!"
                );
                batch_writer
                    .flush_with_error(types::CatchupError::InternalError)
                    .await?;
                return Ok(());
            };

            batch_writer
                .send(types::TransactionWithStatus {
                    transaction_hash: hash.into(),
                    transaction: calimero_primitives::transaction::Transaction {
                        context_id: request.context_id,
                        method: transaction.method.into(),
                        payload: transaction.payload.into(),
                        prior_hash: calimero_primitives::hash::Hash::from(transaction.prior_hash),
                    },
                    status: types::TransactionStatus::Executed,
                })
                .await?;
        }

        for (hash, entry) in self.tx_pool.iter() {
            batch_writer
                .send(types::TransactionWithStatus {
                    transaction_hash: *hash,
                    transaction: calimero_primitives::transaction::Transaction {
                        context_id: request.context_id,
                        method: entry.transaction.method.clone(),
                        payload: entry.transaction.payload.clone(),
                        prior_hash: entry.transaction.prior_hash,
                    },
                    status: types::TransactionStatus::Pending,
                })
                .await?;
        }

        batch_writer.flush().await?;

        Ok(())
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

        let mut last_transaction_hash = request.last_executed_transaction_hash;

        let request = serde_json::to_vec(&types::CatchupStreamMessage::Request(request))?;

        stream
            .send(calimero_network::stream::Message { data: request })
            .await?;

        while let Some(message) = stream.next().await {
            match serde_json::from_slice(&message?.data)? {
                types::CatchupStreamMessage::TransactionsBatch(response) => {
                    let Some(ref context) = context else {
                        eyre::bail!("Received transactions batch for uninitialized context");
                    };

                    for types::TransactionWithStatus {
                        transaction_hash,
                        transaction,
                        status,
                    } in response.transactions
                    {
                        if last_transaction_hash != transaction.prior_hash {
                            eyre::bail!(
                                "Transaction '{}' from the catchup batch doesn't build on last transaction '{}'",
                                transaction_hash,
                                transaction.prior_hash,
                            );
                        };

                        match status {
                            types::TransactionStatus::Pending => match self.typ {
                                calimero_node_primitives::NodeType::Peer => {
                                    self.tx_pool.insert(
                                        chosen_peer,
                                        calimero_primitives::transaction::Transaction {
                                            context_id: context.id,
                                            method: transaction.method,
                                            payload: transaction.payload,
                                            prior_hash: transaction.prior_hash,
                                        },
                                        None,
                                    )?;
                                }
                                calimero_node_primitives::NodeType::Coordinator => {
                                    self.validate_pending_transaction(
                                        last_transaction_hash,
                                        context.clone(),
                                        transaction,
                                        transaction_hash,
                                    )
                                    .await?;
                                }
                            },
                            types::TransactionStatus::Executed => match self.typ {
                                calimero_node_primitives::NodeType::Peer => {
                                    self.execute_transaction(
                                        context.clone(),
                                        transaction,
                                        transaction_hash,
                                    )
                                    .await?;
                                }
                                calimero_node_primitives::NodeType::Coordinator => {
                                    self.persist_transaction(
                                        context.clone(),
                                        transaction.clone(),
                                        transaction_hash,
                                    )?;
                                }
                            },
                        }

                        last_transaction_hash = transaction_hash;
                    }
                }
                types::CatchupStreamMessage::ApplicationChanged(response) => {
                    self.ctx_manager
                        .install_application(&response.application_id, &response.version)
                        .await?;

                    match context {
                        Some(ref mut context_inner) => {
                            self.ctx_manager
                                .update_context_application_id(
                                    context_id,
                                    response.application_id.clone(),
                                )
                                .await?;

                            context_inner.application_id = response.application_id;
                        }
                        None => {
                            let context_inner = calimero_primitives::context::Context {
                                id: context_id,
                                application_id: response.application_id,
                                last_transaction_hash: calimero_primitives::hash::Hash::default(),
                            };

                            self.ctx_manager.add_context(context_inner.clone()).await?;

                            context = Some(context_inner);
                        }
                    }
                }
                types::CatchupStreamMessage::Error(err) => {
                    eyre::bail!(err);
                }
                event => {
                    warn!(?event, "Unexpected event");
                }
            }
        }

        Ok(())
    }
}
