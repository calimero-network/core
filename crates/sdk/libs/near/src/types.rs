use serde_with::base64::Base64;
use serde_with::serde_as;

pub type BlockHeight = u64;
pub type BlockHash = calimero_primitives::hash::Hash;
pub type AccountId = near_account_id::AccountId;
pub type StorageUsage = u64;
pub type Nonce = u64;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum BlockId {
    Height(BlockHeight),
    Hash(BlockHash),
}

#[serde_as]
#[derive(serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
pub struct StoreValue(#[serde_as(as = "Base64")] pub Vec<u8>);

#[serde_as]
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
#[serde(transparent)]
pub struct StoreKey(#[serde_as(as = "Base64")] pub Vec<u8>);

#[serde_as]
#[derive(serde::Serialize, Clone, Debug)]
#[serde(transparent)]
pub struct FunctionArgs(#[serde_as(as = "Base64")] Vec<u8>);
