use std::collections::VecDeque;

use calimero_primitives::identity::ContextIdentity;
use calimero_store::entry::DataType;
use futures_util::{SinkExt, StreamExt};
use tracing::{error, info, warn};

use crate::transaction_pool::TransactionPoolEntry;
use crate::{types, Node};

mod batch;

impl Node {
    pub fn derive_executor_public_key(
        &self,
        context_id: &calimero_primitives::context::ContextId,
        executor_public_key: &[u8; 32],
    ) -> eyre::Result<[u8; 32]> {
        let handle = self.store.handle();

        const CONTEXT_IDENTITIES_SCOPE: [u8; 16] = *b"0000000000000000";

        // Retrieve ContextIdentities
        let identities_key =
            calimero_store::key::Generic::new(CONTEXT_IDENTITIES_SCOPE, **context_id);
        let identities: calimero_primitives::identity::ContextIdentities = handle
            .get(&identities_key)?
            .map(|data: calimero_store::types::GenericData| {
                borsh::from_slice(&data.as_slice()?)
                    .map_err(|e| eyre::eyre!("Failed to deserialize ContextIdentities: {}", e))
            })
            .transpose()?
            .ok_or_else(|| eyre::eyre!("ContextIdentities not found for context {}", context_id))?;

        // Find the matching public key
        identities
            .identities
            .iter()
            .find(|identity| &identity.public_key == executor_public_key)
            .map(|identity| identity.public_key)
            .ok_or_else(|| eyre::eyre!("Executor public key not found in context identities"))
    }

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

        let Some(url) = request.url else {
            eyre::bail!("Path is missing in the request")
        };

        let Some(hash) = request.hash else {
            eyre::bail!("Hash is missing in the request")
        };

        if request
            .application_id
            .map_or(true, |id| id != application_id)
        {
            let application_version = self
                .ctx_manager
                .get_application_latest_version(&application_id)?;

            let message = serde_json::to_vec(&types::CatchupStreamMessage::ApplicationChanged(
                types::CatchupApplicationChanged {
                    application_id,
                    version: application_version,
                    url,
                    hash,
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
                    transaction_hash: hash.into(),
                    transaction: calimero_primitives::transaction::Transaction {
                        context_id: request.context_id,
                        method: transaction.method.into(),
                        payload: transaction.payload.into(),
                        prior_hash: calimero_primitives::hash::Hash::from(transaction.prior_hash),
                        executor_public_key: self
                            .derive_executor_public_key(
                                &request.context_id,
                                &transaction.executor_public_key,
                            )
                            .map_err(|e| {
                                eyre::eyre!("Failed to derive executor public key: {}", e)
                            })?,
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
                        executor_public_key: self
                            .derive_executor_public_key(
                                &request.context_id,
                                &transaction.executor_public_key,
                            )
                            .map_err(|e| {
                                eyre::eyre!("Failed to derive executor public key: {}", e)
                            })?,
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
                    url: None,
                    hash: None,
                },
            ),
            None => (
                None,
                types::CatchupRequest {
                    context_id,
                    application_id: None,
                    last_executed_transaction_hash: calimero_primitives::hash::Hash::default(),
                    batch_size: self.network_client.catchup_config.batch_size,
                    url: None,
                    hash: None,
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
                self.network_client.catchup_config.receive_timeout.into(),
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
                                let executor_public_key = self
                                    .derive_executor_public_key(
                                        &context_.id,
                                        &transaction.executor_public_key,
                                    )
                                    .map_err(|e| {
                                        eyre::eyre!("Failed to derive executor public key: {}", e)
                                    })?;

                                self.tx_pool.insert(
                                    chosen_peer,
                                    calimero_primitives::transaction::Transaction {
                                        context_id: context_.id,
                                        method: transaction.method,
                                        payload: transaction.payload,
                                        prior_hash: transaction.prior_hash,
                                        executor_public_key,
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
                    .is_application_installed(&change.application_id)
                {
                    self.ctx_manager
                        .install_application(
                            &change.application_id,
                            &change.version,
                            &change.url,
                            Some(change.hash.as_str()),
                        )
                        .await?;
                }

                match context {
                    Some(ref mut context_) => {
                        self.ctx_manager
                            .update_context_application_id(
                                context_.id,
                                change.application_id.clone(),
                            )
                            .await?;

                        context_.application_id = change.application_id;
                    }
                    None => {
                        let context_inner = calimero_primitives::context::Context {
                            id: context_id,
                            application_id: change.application_id,
                            last_transaction_hash: calimero_primitives::hash::Hash::default(),
                        };

                        // We don't have the private key during catchup
                        let initial_identity = ContextIdentity {
                            public_key: *context_id,
                            private_key: None,
                        };

                        self.ctx_manager
                            .add_context(context_inner.clone(), initial_identity)
                            .await?;

                        context = Some(context_inner);
                    }
                }
            }
            types::CatchupStreamMessage::Error(err) => {
                error!(?err, "Received error during catchup");
                eyre::bail!(err);
            }
            event => {
                warn!(?event, "Unexpected event");
            }
        }

        Ok(context)
    }
}
