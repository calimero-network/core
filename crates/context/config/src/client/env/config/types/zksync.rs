use serde::{Deserialize, Serialize};
use zksync_web3_rs::eip712::{Eip712Meta, Eip712TransactionRequest};
use zksync_web3_rs::types::{Address, BlockNumber, U256};

#[derive(Debug)]
pub struct ZkSyncTransaction {
    pub to: Address,
    pub value: U256,
    pub meta: Option<Eip712Meta>,
    pub block_number: BlockNumber,
}

impl From<ZkSyncTransaction> for Eip712TransactionRequest {
    fn from(tx: ZkSyncTransaction) -> Self {
        let mut request = Self::new();
        request = request.to(tx.to);
        request = request.value(tx.value);
        if let Some(meta) = tx.meta {
            request = request.custom_data(meta);
        }
        request
    }
}

impl Default for ZkSyncTransaction {
    fn default() -> Self {
        Self {
            to: Address::zero(),
            value: U256::zero(),
            meta: None,
            block_number: BlockNumber::Latest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[allow(dead_code, reason = "Used for type safety and future extensibility")]
pub struct ZkSyncBlockConfig {
    pub block_number: BlockNumber,
    pub confirmations: u64,
}

impl Default for ZkSyncBlockConfig {
    fn default() -> Self {
        Self {
            block_number: BlockNumber::Latest,
            confirmations: 1,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[allow(dead_code, reason = "Used for type safety and future extensibility")]
pub struct ZkSyncGasConfig {
    pub gas_limit: Option<U256>,
    pub max_fee_per_gas: Option<U256>,
    pub max_priority_fee_per_gas: Option<U256>,
}

impl Default for ZkSyncGasConfig {
    fn default() -> Self {
        Self {
            gas_limit: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        }
    }
}
