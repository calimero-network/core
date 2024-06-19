use crate::{
    types::{BlockHash, BlockHeight, BlockId},
    views::{AccountView, ContractCodeView, QueryRequest},
};

#[derive(serde::Serialize, Debug)]
pub struct RpcQueryRequest {
    pub block_id: BlockId,
    #[serde(flatten)]
    pub request: QueryRequest,
}

#[derive(serde::Deserialize, Debug)]
pub struct RpcQueryResponse {
    #[serde(flatten)]
    pub kind: QueryResponseKind,
    pub block_height: BlockHeight,
    pub block_hash: BlockHash,
}

#[derive(serde::Deserialize, Debug)]
#[serde(untagged)]
pub enum QueryResponseKind {
    ViewAccount(AccountView),
    ViewCode(ContractCodeView),
}
