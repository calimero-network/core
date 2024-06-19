use crate::types::{AccountId, BlockHeight, StorageUsage};

#[derive(serde::Serialize, Debug)]
#[serde(tag = "request_type", rename_all = "snake_case")]
pub enum QueryRequest {
    ViewAccount { account_id: AccountId },
    ViewCode { account_id: AccountId },
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct AccountView {
    pub amount: String,
    pub locked: String,
    pub code_hash: String,
    pub storage_usage: StorageUsage,
    pub storage_paid_at: BlockHeight,
}

#[derive(serde::Deserialize, Debug, Clone)]
pub struct ContractCodeView {
    #[serde(rename = "code_base64")]
    pub code: String,
    pub hash: String,
}
