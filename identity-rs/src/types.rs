use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedPublicKey(Vec<u8>);

#[derive(Debug,Clone, Serialize, Deserialize)]
pub struct VerificationMethod {
    pub id: String,
    #[serde(rename = "type")]
    pub algorithm_type: String,
    #[serde(rename = "publicKeyMultibase")]
    pub public_key_multibase: String,
    pub controller: String,
}

#[derive(Debug,Clone, Serialize, Deserialize)]
pub struct DidDocument {
    pub id: String,
    #[serde(rename = "verificationMethod")]
    pub verification_method: Vec<VerificationMethod>,
}

impl DidDocument {
    pub fn new(id: String, verification_method:Vec<VerificationMethod>) -> Self {
        Self {
            id,
            verification_method
         }
    }
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
