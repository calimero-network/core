use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct SerializedPublicKey(Vec<u8>);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationMethod {
    pub id: String,
    #[serde(rename = "type")]
    pub algorithm_type: String,
    #[serde(rename = "publicKeyMultibase")]
    pub public_key_multibase: String,
    pub controller: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidDocument {
    pub id: String,
    pub verification_method: Vec<VerificationMethod>,
}

impl DidDocument {
    pub fn new(id: String, verification_method: Vec<VerificationMethod>) -> Self {
        Self {
            id,
            verification_method,
        }
    }
}

#[derive(Debug, Serialize, Clone, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalletType {
    ETH,
    NEAR,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VerifiableCredentialType {
    Wallet(WalletVerifiableCredential),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename = "camelCase")]
pub struct WalletVerifiableCredential {
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub address: String,
    pub public_key: Vec<u8>,
    pub peer_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename = "camelCase")]
pub struct VerifiableCredential {
    pub id: String,
    pub issuer: String,
    #[serde(rename = "type")]
    pub algorithm_type: AlgorithmType,
    pub credential_subject: VerifiableCredentialType,
    pub proof: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiablePresentation {
    pub challenge: String,
    pub verifiable_credential: VerifiableCredential,
    pub signature: Vec<u8>,
}
