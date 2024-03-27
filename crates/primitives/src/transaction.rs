use serde::{Deserialize, Serialize};

use crate::hash::Hash;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Transaction {
    pub application_id: String,
    pub method: String,
    pub payload: Vec<u8>,
    pub prior_hash: Hash,
}
