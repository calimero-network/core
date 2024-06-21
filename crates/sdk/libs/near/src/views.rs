use std::sync::Arc;

use serde_with::base64::Base64;
use serde_with::serde_as;

use crate::types::{
    AccountId, BlockHeight, FunctionArgs, Nonce, StorageUsage, StoreKey, StoreValue,
};

#[derive(serde::Serialize, Debug)]
#[serde(tag = "request_type", rename_all = "snake_case")]
pub enum QueryRequest {
    ViewAccount {
        account_id: AccountId,
    },
    ViewCode {
        account_id: AccountId,
    },
    ViewState {
        account_id: AccountId,
        #[serde(rename = "prefix_base64")]
        prefix: StoreKey,
        #[serde(default)]
        include_proof: bool,
    },
    ViewAccessKey {
        account_id: AccountId,
        public_key: String,
    },
    ViewAccessKeyList {
        account_id: AccountId,
    },
    CallFunction {
        account_id: AccountId,
        method_name: String,
        #[serde(rename = "args_base64")]
        args: FunctionArgs,
    },
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccountView {
    pub amount: String,
    pub locked: String,
    pub code_hash: String,
    pub storage_usage: StorageUsage,
    pub storage_paid_at: BlockHeight,
}

#[serde_as]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct ContractCodeView {
    #[serde(rename = "code_base64")]
    #[serde_as(as = "Base64")]
    pub code: Vec<u8>,
    pub hash: String,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct StateItem {
    pub key: StoreKey,
    pub value: StoreValue,
}

#[serde_as]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct ViewStateResult {
    pub values: Vec<StateItem>,
    #[serde_as(as = "Vec<Base64>")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof: Vec<Arc<[u8]>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AccessKeyView {
    pub nonce: Nonce,
    pub permission: AccessKeyPermissionView,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub enum AccessKeyPermissionView {
    FunctionCall {
        allowance: Option<String>,
        receiver_id: String,
        method_names: Vec<String>,
    },
    FullAccess,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccessKeyList {
    pub keys: Vec<AccessKeyInfoView>,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccessKeyInfoView {
    pub public_key: String,
    pub access_key: AccessKeyView,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct CallResult {
    pub result: Vec<u8>,
    pub logs: Vec<String>,
}
