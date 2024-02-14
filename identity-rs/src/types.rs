use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedPublicKey(Vec<u8>);

#[derive(Debug, Serialize, Deserialize)]
pub struct VerificationMethod {
    pub id: String,
    #[serde(rename = "type")]
    pub algorithm_type: String,
    #[serde(rename = "publicKeyMultibase")]
    pub public_key_multibase: String,
    pub controller: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidDocument {
    pub id: String,
    pub verification_method: Vec<VerificationMethod>,
}

#[derive(Debug)]
pub enum AlgorithmType {
    Ed25519,
}

impl fmt::Display for AlgorithmType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AlgorithmType::Ed25519 => write!(f, "Ed25519"),
        }
    }
}
