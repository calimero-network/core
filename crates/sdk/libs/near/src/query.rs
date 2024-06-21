use crate::{
    types::{BlockHash, BlockHeight, BlockId},
    views::{
        AccessKeyList, AccessKeyView, AccountView, CallResult, ContractCodeView, QueryRequest,
        ViewStateResult,
    },
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
    ViewState(ViewStateResult),
    AccessKey(AccessKeyView),
    AccessKeyList(AccessKeyList),
    CallResult(CallResult),
}
