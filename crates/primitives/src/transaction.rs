use serde::{Deserialize, Serialize};

use crate::context::ContextId;
use crate::hash::Hash;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct Transaction {
    pub context_id: ContextId,
    pub method: String,
    pub payload: Vec<u8>,
    pub prior_hash: Hash,
    pub executor_public_key: [u8; 32],
}

impl Transaction {
    #[must_use]
    pub fn new(
        context_id: ContextId,
        method: String,
        payload: Vec<u8>,
        prior_hash: Hash,
        executor_public_key: [u8; 32],
    ) -> Self {
        Self {
            context_id,
            method,
            payload,
            prior_hash,
            executor_public_key,
        }
    }
}
