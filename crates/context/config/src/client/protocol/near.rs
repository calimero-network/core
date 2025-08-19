use std::borrow::Cow;
use std::collections::BTreeMap;
use std::{env, time, vec};

pub use near_crypto::SecretKey;
use near_crypto::{InMemorySigner, PublicKey, Signer};
use near_jsonrpc_client::errors::{
    JsonRpcError, JsonRpcServerError, JsonRpcServerResponseStatusError,
};
use near_jsonrpc_client::methods::query::{RpcQueryRequest, RpcQueryResponse};
use near_jsonrpc_client::methods::send_tx::RpcSendTransactionRequest;
use near_jsonrpc_client::methods::tx::RpcTransactionStatusRequest;
use near_jsonrpc_client::{auth, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_jsonrpc_primitives::types::transactions::{RpcTransactionError, TransactionInfo};
use near_primitives::account::id::ParseAccountError;
use near_primitives::action::{Action, FunctionCallAction};
use near_primitives::hash::CryptoHash;
use near_primitives::transaction::{Transaction, TransactionV0};
pub use near_primitives::types::AccountId;
use near_primitives::types::{BlockReference, FunctionArgs};
use near_primitives::views::{
    AccessKeyPermissionView, AccessKeyView, CallResult, FinalExecutionStatus, QueryRequest,
    TxExecutionStatus,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum Near {}

impl Protocol for Near {
    const PROTOCOL: &'static str = "near";
}

impl AssociatedTransport for NearTransport<'_> {
    type Protocol = Near;
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: AccountId,
    pub public_key: PublicKey,
    pub secret_key: SecretKey,
}

mod serde_creds {
    use near_crypto::{PublicKey, SecretKey};
    use near_primitives::types::AccountId;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: AccountId,
        public_key: PublicKey,
        secret_key: SecretKey,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = &'static str;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            'pass: {
                if let SecretKey::ED25519(key) = &creds.secret_key {
                    let mut buf = [0; 32];

                    buf.copy_from_slice(&key.0[..32]);

                    if ed25519_dalek::SigningKey::from_bytes(&buf)
                        .verifying_key()
                        .as_bytes()
                        == &key.0[32..]
                    {
                        break 'pass;
                    }
                } else if creds.public_key == creds.secret_key.public_key() {
                    break 'pass;
                }

                return Err("public key and secret key do not match");
            };

            if creds.account_id.get_account_type().is_implicit() {
                let Ok(public_key) = PublicKey::from_near_implicit_account(&creds.account_id)
                else {
                    return Err("fatal: failed to derive public key from implicit account ID");
                };

                if creds.public_key != public_key {
                    return Err("implicit account ID and public key do not match");
                }
            }

            Ok(Self {
                account_id: creds.account_id,
                public_key: creds.public_key,
                secret_key: creds.secret_key,
            })
        }
    }
}

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

#[derive(Clone, Debug)]
struct Network {
    client: JsonRpcClient,
    account_id: AccountId,
    secret_key: SecretKey,
}

#[derive(Clone, Debug)]
pub struct NearTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> NearTransport<'a> {
    #[must_use]
    pub fn new(config: &NearConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let mut client = JsonRpcClient::connect(network_config.rpc_url.clone());

            if env::var("CALIMERO_TRANSPORT_NEAR_UNIQUE_CONNECTIONS")
                .map_or(false, |v| matches!(&*v, "1" | "true" | "yes"))
            {
                client = client
                    .header(("connection", "close"))
                    .expect("this is a valid header value");
            }

            // Apply NEAR API key authentication if available
            if let Ok(api_key) = env::var("NEAR_API_KEY") {
                client = client.header(auth::ApiKey::new(&api_key).expect("valid API key"));
                client =
                    client.header(auth::Authorization::bearer(&api_key).expect("valid API key"));
            }

            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client,
                    account_id: network_config.account_id.clone(),
                    secret_key: network_config.access_key.clone(),
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum NearError {
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
    #[error("invalid contract ID `{0}`")]
    InvalidContractId(ParseAccountError),
    #[error("access key does not have permission to call contract `{0}`")]
    NotPermittedToCallContract(AccountId),
    #[error(
        "access key does not have permission to call method `{method}` on contract {contract}"
    )]
    NotPermittedToCallMethod { contract: AccountId, method: String },
    #[error("transaction timed out")]
    TransactionTimeout,
    #[error("error while {operation}: {reason}")]
    Custom {
        operation: ErrorOperation,
        reason: String,
    },
}

#[derive(Copy, Clone, Debug, Error)]
#[non_exhaustive]
pub enum ErrorOperation {
    #[error("querying contract")]
    Query,
    #[error("mutating contract")]
    Mutate,
    #[error("fetching account")]
    FetchAccount,
}

impl ProtocolTransport for NearTransport<'_> {
    type Error = NearError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(NearError::UnknownNetwork(request.network_id.into_owned()));
        };

        let contract_id = request
            .contract_id
            .parse()
            .map_err(NearError::InvalidContractId)?;

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
    ) -> Result<Vec<u8>, NearError> {
        let response = self
            .client
            .call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::CallFunction {
                    account_id: contract_id,
                    method_name: method,
                    args: FunctionArgs::from(args),
                },
            })
            .await
            .map_err(|err| NearError::Custom {
                operation: ErrorOperation::Query,
                reason: err.to_string(),
            })?;

        #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
        match response.kind {
            QueryResponseKind::CallResult(CallResult { result, .. }) => Ok(result),
            _ => Err(NearError::InvalidResponse {
                operation: ErrorOperation::Query,
            }),
        }
    }

    async fn mutate(
        &self,
        contract_id: AccountId,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, NearError> {
        let (nonce, block_hash) = self.get_nonce(contract_id.clone(), method.clone()).await?;

        let transaction = Transaction::V0(TransactionV0 {
            signer_id: self.account_id.clone(),
            public_key: self.secret_key.public_key(),
            nonce: nonce.saturating_add(1),
            receiver_id: contract_id,
            block_hash,
            actions: vec![Action::FunctionCall(Box::new(FunctionCallAction {
                method_name: method,
                args,
                gas: 300_000_000_000_000,
                deposit: 0,
            }))],
        });

        let (tx_hash, _) = transaction.get_hash_and_size();

        let sent_at = time::Instant::now();

        let mut response = self
            .client
            .call(RpcSendTransactionRequest {
                signed_transaction: transaction.sign(&Signer::InMemory(
                    InMemorySigner::from_secret_key(
                        self.account_id.clone(),
                        self.secret_key.clone(),
                    ),
                )),
                wait_until: TxExecutionStatus::Final,
            })
            .await;

        let response: near_jsonrpc_client::methods::tx::RpcTransactionResponse = loop {
            match response {
                Ok(response) => break response,
                Err(err) => {
                    #[expect(
                        clippy::wildcard_enum_match_arm,
                        reason = "quite terse, these variants"
                    )]
                    match err {
                        JsonRpcError::ServerError(
                            JsonRpcServerError::ResponseStatusError(
                                JsonRpcServerResponseStatusError::TimeoutError,
                            )
                            | JsonRpcServerError::HandlerError(RpcTransactionError::TimeoutError),
                        ) => {}
                        _ => {
                            return Err(NearError::Custom {
                                operation: ErrorOperation::Mutate,
                                reason: err.to_string(),
                            });
                        }
                    }

                    if sent_at.elapsed().as_secs() > 60 {
                        return Err(NearError::TransactionTimeout);
                    }

                    response = self
                        .client
                        .call(RpcTransactionStatusRequest {
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
            return Err(NearError::InvalidResponse {
                operation: ErrorOperation::Mutate,
            });
        };

        match outcome.into_outcome().status {
            FinalExecutionStatus::SuccessValue(value) => Ok(value),
            FinalExecutionStatus::Failure(error) => Err(NearError::Custom {
                operation: ErrorOperation::Mutate,
                reason: error.to_string(),
            }),
            FinalExecutionStatus::NotStarted | FinalExecutionStatus::Started => {
                Err(NearError::InvalidResponse {
                    operation: ErrorOperation::Mutate,
                })
            }
        }
    }

    async fn get_nonce(
        &self,
        contract_id: AccountId,
        method: String,
    ) -> Result<(u64, CryptoHash), NearError> {
        let response = self
            .client
            .call(RpcQueryRequest {
                block_reference: BlockReference::latest(),
                request: QueryRequest::ViewAccessKey {
                    account_id: self.account_id.clone(),
                    public_key: self.secret_key.public_key().clone(),
                },
            })
            .await
            .map_err(|err| NearError::Custom {
                operation: ErrorOperation::FetchAccount,
                reason: err.to_string(),
            })?;

        let RpcQueryResponse {
            kind: QueryResponseKind::AccessKey(AccessKeyView { nonce, permission }),
            block_hash,
            ..
        } = response
        else {
            return Err(NearError::InvalidResponse {
                operation: ErrorOperation::FetchAccount,
            });
        };

        if let AccessKeyPermissionView::FunctionCall {
            receiver_id,
            method_names,
            ..
        } = permission
        {
            if receiver_id != contract_id {
                return Err(NearError::NotPermittedToCallContract(contract_id));
            }

            if !(method_names.is_empty() || method_names.contains(&method)) {
                return Err(NearError::NotPermittedToCallMethod {
                    contract: contract_id,
                    method,
                });
            }
        }

        Ok((nonce, block_hash))
    }
}
