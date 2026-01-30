mod signature;

pub use signature::{
    canonicalize_manifest, compute_bundle_hash, compute_signing_payload, decode_public_key,
    decode_signature, derive_signer_id_did_key, format_bundle_hash, verify_ed25519,
    verify_manifest_signature, ManifestVerification,
};

use serde::{Deserialize, Serialize};

/// Represents an artifact within a bundle (WASM, ABI, migration)
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleArtifact {
    pub path: String,
    pub hash: Option<String>,
    pub size: u64,
}

/// Display metadata for the application
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleMetadata {
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub license: Option<String>,
}

/// Declarative interfaces (intents) implemented or required by the application
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleInterfaces {
    #[serde(default)]
    pub exports: Vec<String>,
    #[serde(default)]
    pub uses: Vec<String>,
}

/// External links for the application
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleLinks {
    pub frontend: Option<String>,
    pub github: Option<String>,
    pub docs: Option<String>,
}

/// Cryptographic signature of the manifest (CIP-0001 compliant)
///
/// Per CIP-0001, the signature is computed over the SHA-256 hash of the
/// canonical manifest bytes (RFC 8785 JCS) with the `signature` field excluded.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleSignature {
    /// Signature algorithm identifier. MUST be "ed25519" in v0.
    pub algorithm: String,
    /// Ed25519 public key encoded as base64url (no padding).
    pub public_key: String,
    /// Signature over canonical manifest encoded as base64url (no padding).
    pub signature: String,
    /// Optional ISO 8601 timestamp of signing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_at: Option<String>,
}

/// Bundle manifest describing the contents of a bundle archive
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: String,
    pub package: String,
    pub app_version: String,

    /// The signerId (did:key) derived from the signing public key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_id: Option<String>,

    /// Minimum required runtime version (semver).
    pub min_runtime_version: String,

    #[serde(default)]
    pub metadata: Option<BundleMetadata>,

    #[serde(default)]
    pub interfaces: Option<BundleInterfaces>,

    pub wasm: Option<BundleArtifact>,
    pub abi: Option<BundleArtifact>,

    #[serde(default)]
    pub migrations: Vec<BundleArtifact>,

    #[serde(default)]
    pub links: Option<BundleLinks>,

    #[serde(default)]
    pub signature: Option<BundleSignature>,
}
