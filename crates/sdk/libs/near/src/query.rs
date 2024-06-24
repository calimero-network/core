use serde_json::json;

use crate::types::{BlockHash, BlockHeight, BlockId};
use crate::views::{
    AccessKeyList, AccessKeyView, AccountView, CallResult, ContractCodeView, QueryRequest,
    ViewStateResult,
};
use crate::RpcMethod;

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

impl RpcMethod for RpcQueryRequest {
    type Response = RpcQueryResponse;

    fn method_name(&self) -> &str {
        "query"
    }

    fn params(&self) -> Result<serde_json::Value, std::io::Error> {
        Ok(json!(self))
    }
}
