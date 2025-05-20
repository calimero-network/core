use std::borrow::Cow;
use std::collections::BTreeMap;

use alloy::eips::BlockId;
use alloy::network::{Ethereum as EthereumNetwork, EthereumWallet, ReceiptResponse};
use alloy::primitives::{keccak256, Address, Bytes};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum Ethereum {}

impl Protocol for Ethereum {
    const PROTOCOL: &'static str = "ethereum";
}

impl AssociatedTransport for EthereumTransport<'_> {
    type Protocol = Ethereum;
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(try_from = "serde_creds::Credentials")]
pub struct Credentials {
    pub account_id: String,
    pub secret_key: String,
}

mod serde_creds {
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Credentials {
        account_id: String,
        secret_key: String,
    }

    impl TryFrom<Credentials> for super::Credentials {
        type Error = &'static str;

        fn try_from(creds: Credentials) -> Result<Self, Self::Error> {
            Ok(Self {
                account_id: creds.account_id,
                secret_key: creds.secret_key,
            })
        }
    }
}

#[derive(Debug)]
pub struct NetworkConfig {
    pub rpc_url: Url,
    pub account_id: String,
    pub access_key: PrivateKeySigner,
}

#[derive(Debug)]
pub struct EthereumConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    provider: DynProvider<EthereumNetwork>,
}

#[derive(Clone, Debug)]
pub struct EthereumTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> EthereumTransport<'a> {
    #[must_use]
    pub fn new(config: &EthereumConfig<'a>) -> Self {
        let mut networks = BTreeMap::new();

        for (network_id, network_config) in &config.networks {
            let wallet = EthereumWallet::from(network_config.access_key.clone());

            let provider: DynProvider<EthereumNetwork> = ProviderBuilder::new()
                .wallet(wallet)
                .on_http(network_config.rpc_url.clone())
                .erased();

            let _ignored = networks.insert(network_id.clone(), Network { provider });
        }

        Self { networks }
    }
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EthereumError {
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

impl ProtocolTransport for EthereumTransport<'_> {
    type Error = EthereumError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(EthereumError::UnknownNetwork(
                request.network_id.into_owned(),
            ));
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
    ) -> Result<Vec<u8>, EthereumError> {
        let address = contract_id
            .parse::<Address>()
            .map_err(|e| EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;

        let method_selector = &keccak256(method.as_bytes())[..4];

        let call_data = [method_selector, &args].concat();

        let request = TransactionRequest::default()
            .to(address)
            .input(Bytes::from(call_data).into());

        let bytes = self
            .provider
            .call(&request)
            .block(BlockId::latest())
            .await
            .map_err(|e| EthereumError::Custom {
                operation: ErrorOperation::Query,
                reason: format!("Failed to execute eth_call: {}", e),
            })?;

        Ok(bytes.into())
    }

    pub async fn mutate(
        &self,
        contract_id: String,
        method: String,
        args: Vec<u8>,
    ) -> Result<Vec<u8>, EthereumError> {
        let address = contract_id
            .parse::<Address>()
            .map_err(|e| EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: e.to_string(),
            })?;

        let method_selector = &keccak256(method.as_bytes());

        let mut selector = [0u8; 4];
        selector.copy_from_slice(&method_selector[0..4]);

        let mut call_data = Vec::with_capacity(4 + args.len());
        call_data.extend_from_slice(&selector);
        call_data.extend_from_slice(&args);

        let request = TransactionRequest::default()
            .to(address)
            .input(Bytes::from(call_data).into());

        // Send the transaction, wait for it to be confirmed, and get the receipt
        let tx = self
            .provider
            .send_transaction(request.clone())
            .await
            .map_err(|e| EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Failed to send transaction: {}", e),
            })?;

        let receipt = tx
            .with_required_confirmations(1)
            .with_timeout(Some(Duration::from_secs(60)))
            .get_receipt()
            .await
            .map_err(|e| EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Failed to get transaction receipt: {}", e),
            })?;

        if !receipt.status() {
            return Err(EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Transaction failed".to_owned(),
            });
        }

        let block_number = receipt
            .block_number()
            .ok_or_else(|| EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Failed to get block number".to_owned(),
            })?;

        let return_data = self
            .provider
            .call(&request)
            .block((block_number - 1).into())
            .await
            .map_err(|e| EthereumError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Result retrieval failed: {}", e),
            })?;

        Ok(return_data.into())
    }
}
