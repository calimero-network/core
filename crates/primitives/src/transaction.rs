use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub context_id: ContextId,
    pub method: String,
    pub payload: Vec<u8>,
    pub prior_hash: Hash,
}
