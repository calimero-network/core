use serde_with::base64::Base64;
use serde_with::serde_as;

pub type BlockHeight = u64;
pub type BlockHash = calimero_primitives::hash::Hash;
pub type AccountId = near_account_id::AccountId;
pub type StorageUsage = u64;
pub type Nonce = u64;
pub type Balance = u128;
pub type ShardId = u64;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum BlockId {
    Height(BlockHeight),
    Hash(BlockHash),
}

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
