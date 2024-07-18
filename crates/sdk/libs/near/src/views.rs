use std::sync::Arc;

use serde_with::base64::Base64;
use serde_with::{serde_as, DisplayFromStr};

use crate::types::{AccountId, Balance, BlockHeight, BlockId, Nonce, StorageUsage};

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
        public_key: Box<str>,
    },
    ViewAccessKeyList {
        account_id: AccountId,
    },
    CallFunction {
        account_id: AccountId,
        method_name: Box<str>,
        #[serde(rename = "args_base64")]
        args: FunctionArgs,
    },
}

#[serde_as]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccountView {
    #[serde_as(as = "DisplayFromStr")]
    pub amount: Balance,
    #[serde_as(as = "DisplayFromStr")]
    pub locked: Balance,
    pub code_hash: calimero_primitives::hash::Hash,
    pub storage_usage: StorageUsage,
    pub storage_paid_at: BlockHeight,
}

#[serde_as]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct ContractCodeView {
    #[serde(rename = "code_base64")]
    #[serde_as(as = "Base64")]
    pub code: Box<[u8]>,
    pub hash: Box<str>,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct StateItem {
    pub key: StoreKey,
    pub value: StoreValue,
}

#[serde_as]
#[derive(serde::Deserialize, Debug, Clone)]
pub struct ViewStateResult {
    pub values: Box<[StateItem]>,
    #[serde_as(as = "Box<[Base64]>")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof: Box<[Arc<[u8]>]>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct AccessKeyView {
    pub nonce: Nonce,
    pub permission: AccessKeyPermissionView,
}

#[serde_as]
#[derive(Debug, Clone, serde::Deserialize)]
pub enum AccessKeyPermissionView {
    FunctionCall {
        #[serde_as(as = "Option<DisplayFromStr>")]
        allowance: Option<Balance>,
        receiver_id: Box<str>,
        method_names: Box<[Box<str>]>,
    },
    FullAccess,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccessKeyList {
    pub keys: Box<[AccessKeyInfoView]>,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccessKeyInfoView {
    pub public_key: Box<str>,
    pub access_key: AccessKeyView,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct CallResult {
    pub result: Box<[u8]>,
    pub logs: Box<[Box<str>]>,
}

#[serde_as]
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
pub struct StoreValue(#[serde_as(as = "Base64")] pub Box<[u8]>);

#[serde_as]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
pub struct StoreKey(#[serde_as(as = "Base64")] pub Box<[u8]>);

#[serde_as]
#[derive(serde::Serialize, Clone, Debug)]
#[serde(transparent)]
pub struct FunctionArgs(#[serde_as(as = "Base64")] Box<[u8]>);

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReference {
    BlockId(BlockId),
    Finality(Finality),
    SyncCheckpoint(SyncCheckpoint),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCheckpoint {
    Genesis,
    EarliestAvailable,
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone, Debug)]
pub enum Finality {
    #[serde(rename = "optimistic")]
    None,
    #[serde(rename = "near-final")]
    DoomSlug,
    #[serde(rename = "final")]
    #[default]
    Final,
}
