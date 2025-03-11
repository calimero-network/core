use std::borrow::Cow;
use std::collections::BTreeMap;
use std::str::FromStr;

use alloy::network::EthereumWallet;
use alloy::primitives::{keccak256, Address, Bytes};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::client::{ClientBuilder, ReqwestClient};
use alloy::rpc::types::{TransactionInput, TransactionRequest};
use alloy::signers::local::PrivateKeySigner;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum Evm {}

impl Protocol for Evm {
    const PROTOCOL: &'static str = "evm";
}

impl AssociatedTransport for EvmTransport<'_> {
    type Protocol = Evm;
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: String,
    pub public_key: String,
    pub secret_key: String,
}

mod serde_creds {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: String,
        public_key: String,
        secret_key: String,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = &'static str;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
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
    pub account_id: String,
    pub access_key: String,
}

#[derive(Debug)]
pub struct EvmConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    client: ReqwestClient,
    rpc_url: String,
    secret_key: String,
}

#[derive(Clone, Debug)]
pub struct EvmTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> EvmTransport<'a> {
    #[must_use]
    pub fn new(config: &EvmConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let _ignored = networks.insert(
                network_id.clone(),
                Network {
                    client: ClientBuilder::default().http(network_config.rpc_url.clone()),
                    rpc_url: network_config.rpc_url.clone().to_string(),
                    secret_key: network_config.access_key.clone(),
                },
            );
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EvmError {
    #[error("unknown network `{0}`")]
    UnknownNetwork(String),
    #[error("invalid response from RPC while {operation}")]
    InvalidResponse { operation: ErrorOperation },
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
}

impl ProtocolTransport for EvmTransport<'_> {
    type Error = EvmError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(EvmError::UnknownNetwork(request.network_id.into_owned()));
        };

        let contract_id = request.contract_id.into_owned();

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
        contract_id: String,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, EvmError> {
        let address = contract_id
            .parse::<Address>()
            .map_err(|e| EvmError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;

        let method_selector = &keccak256(method.as_bytes())[..4];

        let call_data = [method_selector, &args].concat();

        let params = vec![
            json!({
                "to": address,
                "data": Bytes::from(call_data)
            }),
            json!("latest"),
        ];

        let response: String =
            self.client
                .request("eth_call", params)
                .await
                .map_err(|e| EvmError::Custom {
                    operation: ErrorOperation::Query,
                    reason: format!("Failed to execute eth_call: {}", e),
                })?;

        let bytes = response.as_bytes().to_vec();

        Ok(bytes)
    }

    async fn mutate(
        &self,
        contract_id: String,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, EvmError> {
        let signer: PrivateKeySigner = PrivateKeySigner::from_str(&self.secret_key).unwrap();
        let wallet = EthereumWallet::from(signer);

        let provider =
            ProviderBuilder::new()
                .wallet(wallet)
                .on_http(Url::parse(&self.rpc_url).map_err(|e| EvmError::Custom {
                    operation: ErrorOperation::Mutate,
                    reason: e.to_string(),
                })?);

        let address = contract_id
            .parse::<Address>()
            .map_err(|e| EvmError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;

        let method_selector = &keccak256(method.as_bytes());

        let mut selector = [0u8; 4];
        selector.copy_from_slice(&method_selector[0..4]);

        let mut call_data = Vec::with_capacity(4 + args.len());
        call_data.extend_from_slice(&selector);
        call_data.extend_from_slice(&args);

        let tx = TransactionRequest::default()
            .to(address)
            .input(TransactionInput {
                input: Some(Bytes::from(call_data)),
                data: None,
            });

        let tx_builder = provider
            .send_transaction(tx)
            .await
            .map_err(|e| EvmError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;
        let tx_hash = tx_builder.tx_hash();

        let mut receipt = None;

        // Wait for the transaction to be mined
        for _ in 0..30 {
            let receipt_params = vec![json!(tx_hash)];
            let result: Option<serde_json::Value> = self
                .client
                .request("eth_getTransactionReceipt", receipt_params)
                .await
                .map_err(|e| EvmError::Custom {
                    operation: ErrorOperation::Mutate,
                    reason: e.to_string(),
                })?;

            if let Some(r) = result {
                receipt = Some(r);
                break;
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }

        let Some(receipt) = receipt else {
            return Err(EvmError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Transaction wasn't mined within timeout period".to_string(),
            });
        };

        let status = receipt
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("0x0");

        if status == "0x1" {
            return Ok(Vec::new());
        }

        Err(EvmError::Custom {
            operation: ErrorOperation::Mutate,
            reason: format!("Transaction failed with status: {}.", status),
        })
    }
}
