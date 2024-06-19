pub type BlockHeight = u64;
pub type BlockHash = String;
pub type AccountId = String;
pub type StorageUsage = u64;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(untagged)]
pub enum BlockId {
    Height(BlockHeight),
    Hash(BlockHash),
}
