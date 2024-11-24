use core::str::FromStr;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use starknet::accounts::{Account, ConnectedAccount, ExecutionEncoding, SingleOwnerAccount};
use starknet::core::codec::Decode;
use starknet::core::types::{
    BlockId, BlockTag, Call, ExecutionResult, Felt, FunctionCall, TransactionFinalityStatus,
};
use starknet::core::utils::get_selector_from_name;
use starknet::providers::jsonrpc::HttpTransport;
use starknet::providers::{JsonRpcClient, Provider, Url};
use starknet::signers::{LocalWallet, SigningKey};
use thiserror::Error;

use super::Protocol;
use crate::client::env::proxy::starknet::StarknetProposalWithApprovals;
use crate::client::transport::{AssociatedTransport, Operation, Transport, TransportRequest};

#[derive(Copy, Clone, Debug)]
pub enum Starknet {}

impl Protocol for Starknet {
    const PROTOCOL: &'static str = "starknet";
}

impl AssociatedTransport for StarknetTransport<'_> {
    type Protocol = Starknet;
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: Felt,
    pub public_key: Felt,
    pub secret_key: Felt,
}

mod serde_creds {
    use core::str::FromStr;

    use serde::{Deserialize, Serialize};
    use starknet_crypto::Felt;
    use starknet_types_core::felt::FromStrError;
    use thiserror::Error;

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        secret_key: String,
        public_key: String,
        account_id: String,
    }

    #[derive(Debug, Error)]
    pub enum CredentialsError {
        #[error("failed to parse Felt from string")]
        ParseError(#[from] FromStrError),
        #[error("public key extracted from secret key does not match the provided public key")]
        PublicKeyMismatch,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = CredentialsError;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            let secret_key_felt = Felt::from_str(&creds.secret_key)
                .map_err(|_| CredentialsError::ParseError(FromStrError))?;
            let public_key_felt = Felt::from_str(&creds.public_key)
                .map_err(|_| CredentialsError::ParseError(FromStrError))?;
            let extracted_public_key = starknet_crypto::get_public_key(&secret_key_felt);

            if public_key_felt != extracted_public_key {
                return Err(CredentialsError::PublicKeyMismatch);
            }

            let account_id_felt = Felt::from_str(&creds.account_id)
                .map_err(|_| CredentialsError::ParseError(FromStrError))?;

            Ok(Self {
                account_id: account_id_felt,
                public_key: public_key_felt,
                secret_key: secret_key_felt,
            })
        }
    }
}

#[derive(Debug)]
pub struct NetworkConfig {
    pub rpc_url: Url,
    pub account_id: Felt,
    pub access_key: Felt,
}

#[derive(Debug)]
pub struct StarknetConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    client: Arc<JsonRpcClient<HttpTransport>>,
    account_id: Felt,
    secret_key: Felt,
}

#[derive(Clone, Debug)]
pub struct StarknetTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> StarknetTransport<'a> {
    #[must_use]
    pub fn new(config: &StarknetConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let client = JsonRpcClient::new(HttpTransport::new(network_config.rpc_url.clone()));
            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client: client.into(),
                    account_id: network_config.account_id,
                    secret_key: network_config.access_key,
                },
            );
        }
        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StarknetError {
    #[error("unsupported protocol: {0}")]
    UnsupportedProtocol(String),
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
    #[error("invalid contract ID `{0}`")]
    InvalidContractId(String),
    #[error("invalid method name `{0}`")]
    InvalidMethodName(String),
    #[error("access key does not have permission to call contract `{0}`")]
    NotPermittedToCallContract(String),
    #[error(
        "access key does not have permission to call method `{method}` on contract {contract}"
    )]
    NotPermittedToCallMethod { contract: String, method: String },
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
    #[error("fetching nonce")]
    FetchNonce,
}

impl Transport for StarknetTransport<'_> {
    type Error = StarknetError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        if request.protocol != Starknet::PROTOCOL {
            return Err(StarknetError::UnsupportedProtocol(
                request.protocol.into_owned(),
            ));
        }

        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(StarknetError::UnknownNetwork(
                request.network_id.into_owned(),
            ));
        };

        let contract_id = request.contract_id.as_ref();

        match request.operation {
            Operation::Read { method } => {
                let response = network.query(contract_id, &method, payload).await?;
                Ok(response)
            }
            Operation::Write { method } => {
                let response = network.mutate(contract_id, &method, payload).await?;
                Ok(response)
            }
        }
    }
}

impl Network {
    async fn query(
        &self,
        contract_id: &str,
        method: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, StarknetError> {
        let contract_id = Felt::from_str(contract_id)
            .map_err(|_| StarknetError::InvalidContractId(contract_id.to_owned()))?;

        let entry_point_selector = get_selector_from_name(method)
            .map_err(|_| StarknetError::InvalidMethodName(method.to_owned()))?;

        let calldata: Vec<Felt> = if args.is_empty() {
            vec![]
        } else {
            args.chunks_exact(32)
                .map(|chunk| {
                    let chunk_array: [u8; 32] = chunk.try_into().expect("chunk should be 32 bytes");
                    Felt::from_bytes_be(&chunk_array)
                })
                .collect()
        };

        let function_call = FunctionCall {
            contract_address: contract_id,
            entry_point_selector,
            calldata,
        };

        let response = self
            .client
            .call(&function_call, BlockId::Tag(BlockTag::Latest))
            .await;

        response.map_or(
            Err(StarknetError::InvalidResponse {
                operation: ErrorOperation::Query,
            }),
            |result| {
                Ok(result
                    .into_iter()
                    .map(|felt| felt.to_bytes_be())
                    .flatten()
                    .collect::<Vec<u8>>())
            },
        )
    }

    async fn mutate(
        &self,
        contract_id: &str,
        method: &str,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, StarknetError> {
        let sender_address: Felt = self.account_id;
        let secret_key: Felt = self.secret_key;
        let contract_id = Felt::from_str(contract_id)
            .map_err(|_| StarknetError::InvalidContractId(contract_id.to_owned()))?;

        let entry_point_selector = get_selector_from_name(method)
            .map_err(|_| StarknetError::InvalidMethodName(method.to_owned()))?;

        let calldata: Vec<Felt> = if args.is_empty() {
            vec![]
        } else {
            args.chunks_exact(32)
                .map(|chunk| {
                    let chunk_array: [u8; 32] = chunk.try_into().expect("chunk should be 32 bytes");
                    Felt::from_bytes_be(&chunk_array)
                })
                .collect()
        };

        let current_network = match self.client.chain_id().await {
            Ok(chain_id) => chain_id,
            Err(e) => {
                return Err(StarknetError::Custom {
                    operation: ErrorOperation::Query,
                    reason: e.to_string(),
                })
            }
        };

        let relayer_signing_key = SigningKey::from_secret_scalar(secret_key);
        let relayer_wallet = LocalWallet::from(relayer_signing_key);
        let mut account = SingleOwnerAccount::new(
            Arc::clone(&self.client),
            relayer_wallet,
            sender_address,
            current_network,
            ExecutionEncoding::New,
        );

        let _ = account.set_block_id(BlockId::Tag(BlockTag::Pending));

        let response = account
            .execute_v1(vec![Call {
                to: contract_id,
                selector: entry_point_selector,
                calldata,
            }])
            .send()
            .await
            .map_err(|e| StarknetError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Failed to send transaction: {}", e),
            })?;

        let sent_at = Instant::now();
        let timeout = Duration::from_secs(60); // Same 60-second timeout as NEAR

        let receipt = loop {
            match account
                .provider()
                .get_transaction_receipt(response.transaction_hash)
                .await
            {
                Ok(receipt) => {
                    if let starknet::core::types::TransactionReceipt::Invoke(invoke_receipt) =
                        &receipt.receipt
                    {
                        if matches!(
                            invoke_receipt.finality_status,
                            TransactionFinalityStatus::AcceptedOnL2
                                | TransactionFinalityStatus::AcceptedOnL1
                        ) {
                            break receipt;
                        }

                        if sent_at.elapsed() > timeout {
                            return Err(StarknetError::TransactionTimeout);
                        }
                        continue;
                    }
                }
                Err(err) => {
                    return Err(StarknetError::Custom {
                        operation: ErrorOperation::Mutate,
                        reason: err.to_string(),
                    });
                }
            }
        };

        // Process the receipt
        match receipt.receipt {
            starknet::core::types::TransactionReceipt::Invoke(invoke_receipt) => {
                match invoke_receipt.execution_result {
                    ExecutionResult::Succeeded => {
                        // Process events and return result
                        for event in invoke_receipt.events.iter() {
                            if event.from_address == contract_id {
                                let result = StarknetProposalWithApprovals::decode(&event.data)
                                    .map_err(|e| StarknetError::Custom {
                                        operation: ErrorOperation::Query,
                                        reason: format!("Failed to decode event: {:?}", e),
                                    })?;
                                let mut encoded = vec![0u8; 32];
                                encoded.extend_from_slice(&result.proposal_id.0.high.to_bytes_be());
                                encoded.extend_from_slice(&result.proposal_id.0.low.to_bytes_be());
                                encoded.extend_from_slice(&result.num_approvals.to_bytes_be());
                                return Ok(encoded);
                            }
                        }
                        Ok(vec![])
                    }
                    ExecutionResult::Reverted { reason } => Err(StarknetError::Custom {
                        operation: ErrorOperation::Mutate,
                        reason: format!("Transaction reverted: {}", reason),
                    }),
                }
            }
            _ => Ok(vec![0]),
        }
    }
}
