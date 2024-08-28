use calimero_primitives::hash::Hash;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error as ThisError;

use crate::types::{AccountId, BlockHash, BlockHeight, BlockId, ShardId};
use crate::views::{
    AccessKeyList, AccessKeyView, AccountView, BlockReference, CallResult, ContractCodeView,
    QueryRequest, ViewStateResult,
};
use crate::RpcMethod;

#[derive(Debug, Serialize)]
pub struct RpcQueryRequest {
    pub block_id: BlockId,
    #[serde(flatten)]
    pub request: QueryRequest,
}

#[derive(Debug, Deserialize)]
pub struct RpcQueryResponse {
    #[serde(flatten)]
    pub kind: QueryResponseKind,
    pub block_height: BlockHeight,
    pub block_hash: BlockHash,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum QueryResponseKind {
    ViewAccount(AccountView),
    ViewCode(ContractCodeView),
    ViewState(ViewStateResult),
    AccessKey(AccessKeyView),
    AccessKeyList(AccessKeyList),
    CallResult(CallResult),
}

impl RpcMethod for RpcQueryRequest {
    type Response = RpcQueryResponse;
    type Error = RpcQueryError;

    fn method_name(&self) -> &str {
        "query"
    }

    fn params(&self) -> Value {
        json!(self)
    }
}

#[derive(Debug, Deserialize, Serialize, ThisError)]
#[serde(tag = "name", content = "info", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RpcQueryError {
    #[error("There are no fully synchronized blocks on the node yet")]
    NoSyncedBlocks,
    #[error("The node does not track the shard ID {requested_shard_id}")]
    UnavailableShard { requested_shard_id: ShardId },
    #[error(
        "The data for block #{block_height} is garbage collected on this node, use an archival node to fetch historical data"
    )]
    GarbageCollectedBlock {
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error("Block either has never been observed on the node or has been garbage collected: {block_reference:?}")]
    UnknownBlock { block_reference: BlockReference },
    #[error("Account ID {requested_account_id} is invalid")]
    InvalidAccount {
        requested_account_id: AccountId,
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error("account {requested_account_id} does not exist while viewing")]
    UnknownAccount {
        requested_account_id: AccountId,
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error(
        "Contract code for contract ID #{contract_account_id} has never been observed on the node"
    )]
    NoContractCode {
        contract_account_id: AccountId,
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error("State of contract {contract_account_id} is too large to be viewed")]
    TooLargeContractState {
        contract_account_id: AccountId,
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error("Access key for public key {public_key} has never been observed on the node")]
    UnknownAccessKey {
        public_key: String,
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error("Function call returned an error: {vm_error}")]
    ContractExecutionError {
        vm_error: String,
        block_height: BlockHeight,
        block_hash: Hash,
    },
    #[error("The node reached its limits. Try again later. More details: {error_message}")]
    InternalError { error_message: String },
}
