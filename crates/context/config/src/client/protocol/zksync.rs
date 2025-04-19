use std::borrow::Cow;
use std::collections::BTreeMap;

use alloy::eips::BlockId;
use alloy::network::{Ethereum as EthereumNetwork, EthereumWallet, ReceiptResponse};
use alloy::primitives::{keccak256, Address, Bytes};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use thiserror::Error;
use tokio::time::Duration;
use zksync_web3_rs::prelude::*;

use super::Protocol;
use crate::client::transport::{
    AssociatedTransport, Operation, ProtocolTransport, TransportRequest,
};

#[derive(Copy, Clone, Debug)]
pub enum ZkSync {}

impl Protocol for ZkSync {
    const PROTOCOL: &'static str = "zksync";
}

impl AssociatedTransport for ZkSyncTransport<'_> {
    type Protocol = ZkSync;
}

// Reuse Ethereum's Credentials type
pub use crate::client::protocol::ethereum::Credentials;

// Reuse Ethereum's NetworkConfig type
pub use crate::client::protocol::ethereum::NetworkConfig;

#[derive(Debug)]
pub struct ZkSyncConfig<'a> {
    pub networks: BTreeMap<Cow<'a, str>, NetworkConfig>,
}

#[derive(Clone, Debug)]
struct Network {
    provider: DynProvider<EthereumNetwork>,
}

#[derive(Clone, Debug)]
pub struct ZkSyncTransport<'a> {
    networks: BTreeMap<Cow<'a, str>, Network>,
}

impl<'a> ZkSyncTransport<'a> {
    #[must_use]
    pub fn new(config: &ZkSyncConfig<'a>) -> Self {
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

// Reuse Ethereum's ErrorOperation type
pub use crate::client::protocol::ethereum::ErrorOperation;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ZkSyncError {
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

impl ProtocolTransport for ZkSyncTransport<'_> {
    type Error = ZkSyncError;

    async fn send(
        &self,
        request: TransportRequest<'_>,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, Self::Error> {
        let Some(network) = self.networks.get(&request.network_id) else {
            return Err(ZkSyncError::UnknownNetwork(
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
    ) -> Result<Vec<u8>, ZkSyncError> {
        let address = contract_id
            .parse::<Address>()
            .map_err(|e| ZkSyncError::Custom {
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
            .map_err(|e| ZkSyncError::Custom {
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
    ) -> Result<Vec<u8>, ZkSyncError> {
        let address = contract_id
            .parse::<Address>()
            .map_err(|e| ZkSyncError::Custom {
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
            .map_err(|e| ZkSyncError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Failed to send transaction: {}", e),
            })?;

        let receipt = tx
            .with_required_confirmations(1)
            .with_timeout(Some(Duration::from_secs(60)))
            .get_receipt()
            .await
            .map_err(|e| ZkSyncError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Failed to get transaction receipt: {}", e),
            })?;

        if !receipt.status() {
            return Err(ZkSyncError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Transaction failed".to_owned(),
            });
        }

        let block_number = receipt
            .block_number()
            .ok_or_else(|| ZkSyncError::Custom {
                operation: ErrorOperation::Mutate,
                reason: "Failed to get block number".to_owned(),
            })?;

        let return_data = self
            .provider
            .call(&request)
            .block((block_number - 1).into())
            .await
            .map_err(|e| ZkSyncError::Custom {
                operation: ErrorOperation::Mutate,
                reason: format!("Result retrieval failed: {}", e),
            })?;

        Ok(return_data.into())
    }
} 