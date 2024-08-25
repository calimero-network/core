use calimero_primitives::hash::Hash;
use serde::{Deserialize, Serialize};

pub type BlockHeight = u64;
pub type BlockHash = Hash;
pub type AccountId = near_account_id::AccountId;
pub type StorageUsage = u64;
pub type Nonce = u64;
pub type Balance = u128;
pub type ShardId = u64;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BlockId {
    Height(BlockHeight),
    Hash(BlockHash),
}
