#![allow(clippy::exhaustive_structs, reason = "TODO: Allowed until reviewed")]

use core::str::FromStr;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use starknet_core::types::{BlockId, BlockTag, Felt, FunctionCall};
use starknet_core::utils::get_selector_from_name;
use starknet_providers::jsonrpc::HttpTransport;
use starknet_providers::{JsonRpcClient, Provider, Url};
use thiserror::Error;

use super::{Operation, Transport, TransportRequest};

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

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        secret_key: String,
        public_key: String,
        account_id: String,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = FromStrError;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            let public_key_felt = Felt::from_str(&creds.public_key)?;
            let secret_key_felt = Felt::from_str(&creds.secret_key)?;
            let account_id_felt = Felt::from_str(&creds.account_id)?;

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
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
    #[error("invalid contract ID `{0}`")]
    InvalidContractId(String),
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
}

impl Transport for StarknetTransport<'_> {
    type Error = StarknetError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(StarknetError::UnknownNetwork(
                request.network_id.into_owned(),
            ));
        };

        let contract_id = request.contract_id.as_ref();

        match request.operation {
            Operation::Read { method } => network.query(contract_id, &method, payload).await,
            Operation::Write { .. } => Ok(vec![]),
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
            .unwrap_or_else(|_| panic!("Failed to convert contract id to felt type"));

        let entry_point_selector = get_selector_from_name(method)
            .unwrap_or_else(|_| panic!("Failed to convert method name to entry point selector"));

        let calldata: Vec<Felt> = if args.is_empty() {
            vec![]
        } else {
            args.chunks(32)
                .map(|chunk| {
                    let mut padded_chunk = [0u8; 32];
                    for (i, byte) in chunk.iter().enumerate() {
                        padded_chunk[i] = *byte;
                    }
                    Felt::from_bytes_be(&padded_chunk)
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
                    .flat_map(|felt| felt.to_bytes_be().to_vec())
                    .collect::<Vec<u8>>())
            },
        )
    }
}
