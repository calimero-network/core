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
