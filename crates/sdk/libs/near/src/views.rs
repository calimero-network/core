use std::sync::Arc;

use calimero_primitives::hash::Hash;
use serde::{Deserialize, Serialize};
use serde_with::base64::Base64;
use serde_with::{serde_as, DisplayFromStr};

use crate::types::{AccountId, Balance, BlockHeight, BlockId, Nonce, StorageUsage};

#[derive(Debug, Serialize)]
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
#[derive(Clone, Debug, Deserialize)]
pub struct AccountView {
    #[serde_as(as = "DisplayFromStr")]
    pub amount: Balance,
    #[serde_as(as = "DisplayFromStr")]
    pub locked: Balance,
    pub code_hash: Hash,
    pub storage_usage: StorageUsage,
    pub storage_paid_at: BlockHeight,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize)]
pub struct ContractCodeView {
    #[serde(rename = "code_base64")]
    #[serde_as(as = "Base64")]
    pub code: Box<[u8]>,
    pub hash: Box<str>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StateItem {
    pub key: StoreKey,
    pub value: StoreValue,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize)]
pub struct ViewStateResult {
    pub values: Box<[StateItem]>,
    #[serde_as(as = "Box<[Base64]>")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof: Box<[Arc<[u8]>]>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccessKeyView {
    pub nonce: Nonce,
    pub permission: AccessKeyPermissionView,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize)]
pub enum AccessKeyPermissionView {
    FunctionCall {
        #[serde_as(as = "Option<DisplayFromStr>")]
        allowance: Option<Balance>,
        receiver_id: Box<str>,
        method_names: Box<[Box<str>]>,
    },
    FullAccess,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccessKeyList {
    pub keys: Box<[AccessKeyInfoView]>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AccessKeyInfoView {
    pub public_key: Box<str>,
    pub access_key: AccessKeyView,
}

#[derive(Clone, Debug, Deserialize)]
pub struct CallResult {
    pub result: Box<[u8]>,
    pub logs: Box<[Box<str>]>,
}

#[serde_as]
#[derive(Clone, Debug, Deserialize)]
#[serde(transparent)]
pub struct StoreValue(#[serde_as(as = "Base64")] pub Box<[u8]>);

#[serde_as]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(transparent)]
pub struct StoreKey(#[serde_as(as = "Base64")] pub Box<[u8]>);

#[serde_as]
#[derive(Clone, Debug, Serialize)]
#[serde(transparent)]
pub struct FunctionArgs(#[serde_as(as = "Base64")] Box<[u8]>);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReference {
    BlockId(BlockId),
    Finality(Finality),
    SyncCheckpoint(SyncCheckpoint),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncCheckpoint {
    Genesis,
    EarliestAvailable,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub enum Finality {
    #[serde(rename = "optimistic")]
    None,
    #[serde(rename = "near-final")]
    DoomSlug,
    #[serde(rename = "final")]
    #[default]
    Final,
}
