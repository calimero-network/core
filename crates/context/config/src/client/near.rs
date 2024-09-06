use std::borrow::Cow;
use std::collections::BTreeMap;
use std::time;

pub use near_crypto::SecretKey;
use near_crypto::{InMemorySigner, Signer};
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::{QueryResponseKind, RpcQueryResponse};
use near_jsonrpc_primitives::types::transactions::{RpcTransactionError, TransactionInfo};
use near_primitives::action::{Action, FunctionCallAction};
use near_primitives::transaction::{Transaction, TransactionV0};
pub use near_primitives::types::AccountId;
use near_primitives::types::{BlockReference, FunctionArgs};
use near_primitives::views::{
    AccessKeyPermissionView, AccessKeyView, CallResult, FinalExecutionStatus, QueryRequest,
    TxExecutionStatus,
};
use thiserror::Error;
use url::Url;

use super::{Operation, Transport, TransportRequest};

#[derive(Debug)]
pub struct NetworkConfig {
    pub rpc_url: Url,
    pub account_id: AccountId,
    pub access_key: SecretKey,
}

#[derive(Debug)]
pub struct NearConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Debug)]
struct Network {
    client: JsonRpcClient,
    account_id: AccountId,
    access_key: SecretKey,
}

#[derive(Debug)]
pub struct NearTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> NearTransport<'a> {
    pub fn new(config: &NearConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let client = JsonRpcClient::connect(network_config.rpc_url.clone());

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client,
                    account_id: network_config.account_id.clone(),
                    access_key: network_config.access_key.clone(),
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("unknown network: {0}")]
    UnknownNetwork(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: String },
    #[error("invalid contract ID: {0}")]
    InvalidContractId(near_primitives::account::id::ParseAccountError),
    #[error("failed while {operation}: {reason}")]
    Custom {
        operation: &'static str,
        reason: String,
    },
}

impl Transport for NearTransport<'_> {
    type Error = Error;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(Error::UnknownNetwork(request.network_id.into_owned()));
        };

        let contract_id = request
            .contract_id
            .parse()
            .map_err(Error::InvalidContractId)?;

        match request.operation {
            Operation::Read { method } => {
                network
                    .query(contract_id, method.into_owned(), payload)
                    .await
            }
            Operation::Write { method } => {
                network
                    .mutate(contract_id, method.into_owned(), payload)
                    .await
            }
        }
    }
}

impl Network {
    async fn query(
        &self,
        contract_id: AccountId,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, Error> {
        let response = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: contract_id,
                    method_name: method,
                    args: FunctionArgs::from(args),
                },
            })
            .await
            .map_err(|err| Error::Custom {
                operation: "querying contract",
                reason: err.to_string(),
            })?;

        match response.kind {
            QueryResponseKind::CallResult(CallResult { result, logs: _ }) => Ok(result),
            _ => Err(Error::InvalidResponse {
                operation: "querying contract".to_owned(),
            }),
        }
    }

    async fn mutate(
        &self,
        contract_id: AccountId,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, Error> {
        let response = self
            .client
            .call(methods::query::RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccessKey {
                    account_id: self.account_id.clone(),
                    public_key: self.access_key.public_key().clone(),
                },
            })
            .await
            .map_err(|err| Error::Custom {
                operation: "fetching account",
                reason: err.to_string(),
            })?;

        let (nonce, permission, block_hash) = match response {
            RpcQueryResponse {
                kind: QueryResponseKind::AccessKey(AccessKeyView { nonce, permission }),
                block_hash,
                block_height: _,
            } => (nonce, permission, block_hash),
            _ => {
                return Err(Error::InvalidResponse {
                    operation: "fetching account".to_owned(),
                })
            }
        };

        if let AccessKeyPermissionView::FunctionCall {
            allowance: _,
            receiver_id,
            method_names,
        } = permission
        {
            if receiver_id != contract_id {
                return Err(Error::Custom {
                    operation: "mutating contract",
                    reason: format!(
                        "access key does not have permission to call contract: {}",
                        contract_id
                    ),
                });
            }

            if !(method_names.is_empty() || method_names.contains(&method)) {
                return Err(Error::Custom {
                    operation: "mutating contract",
                    reason: format!(
                        "access key does not have permission to call method on contract: {}",
                        method
                    ),
                });
            }
        }

        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key: self.access_key.public_key().clone(),
            nonce: nonce + 1,
            receiver_id: contract_id,
            block_hash,
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: method,
                args,
                gas: 100_000_000_000_000, // 100 TeraGas
                deposit: 0,
            }))],
        });

        let (tx_hash, _) = transaction.get_hash_and_size();

        let sent_at = time::Instant::now();

        let mut response = self
            .client
            .call(methods::send_tx::RpcSendTransactionRequest {
                signed_transaction: transaction.sign(&Signer::InMemory(
                    InMemorySigner::from_secret_key(
                        self.account_id.clone(),
                        self.access_key.clone(),
                    ),
                )),
                wait_until: TxExecutionStatus::Final,
            })
            .await;

        let response = loop {
            match response {
                Ok(response) => break response,
                Err(err) => {
                    let Some(RpcTransactionError::TimeoutError) = err.handler_error() else {
                        return Err(Error::Custom {
                            operation: "mutating contract",
                            reason: err.to_string(),
                        });
                    };

                    if sent_at.elapsed().as_secs() > 60 {
                        return Err(Error::Custom {
                            operation: "mutating contract",
                            reason: "transaction timed out".to_owned(),
                        });
                    }

                    response = self
                        .client
                        .call(methods::tx::RpcTransactionStatusRequest {
                            transaction_info: TransactionInfo::TransactionId {
                                tx_hash,
                                sender_account_id: self.account_id.clone(),
                            },
                            wait_until: TxExecutionStatus::Final,
                        })
                        .await;
                }
            }
        };

        let Some(outcome) = response.final_execution_outcome else {
            return Err(Error::InvalidResponse {
                operation: "mutating contract".to_owned(),
            });
        };

        match outcome.into_outcome().status {
            FinalExecutionStatus::SuccessValue(value) => Ok(value),
            FinalExecutionStatus::Failure(error) => Err(Error::Custom {
                operation: "mutating contract",
                reason: error.to_string(),
            }),
            _ => Err(Error::InvalidResponse {
                operation: "mutating contract".to_owned(),
            }),
        }
    }
}
