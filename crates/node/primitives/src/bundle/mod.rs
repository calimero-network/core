mod signature;

pub use signature::{
    canonicalize_manifest, compute_bundle_hash, compute_signing_payload, decode_public_key,
    decode_signature, derive_signer_id_did_key, format_bundle_hash, sign_manifest_json,
    verify_ed25519, verify_manifest_signature, ManifestVerification,
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

/// Cryptographic signature of the manifest
///
/// The signature is computed over the SHA-256 hash of the canonical manifest bytes (RFC 8785 JCS) with the `signature` field excluded.
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

/// A named service within a multi-service bundle.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleService {
    pub name: String,
    pub wasm: BundleArtifact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<BundleArtifact>,
}

/// Bundle manifest describing the contents of a bundle archive.
///
/// Supports two formats:
/// - **Single-service** (backward compat): `wasm` + optional `abi` fields
/// - **Multi-service**: `services` array with named WASM modules
///
/// If `services` is present and non-empty, it takes priority over `wasm`/`abi`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: String,
    pub package: String,
    pub app_version: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer_id: Option<String>,

    pub min_runtime_version: String,

    #[serde(default)]
    pub metadata: Option<BundleMetadata>,

    #[serde(default)]
    pub interfaces: Option<BundleInterfaces>,

    /// Single-service WASM (backward compat). Ignored when `services` is non-empty.
    pub wasm: Option<BundleArtifact>,
    /// Single-service ABI (backward compat). Ignored when `services` is non-empty.
    pub abi: Option<BundleArtifact>,

    /// Named services. When present, each context specifies which service it runs.
    /// If empty/absent, the bundle is single-service and uses `wasm`/`abi` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<BundleService>>,

    #[serde(default)]
    pub migrations: Vec<BundleArtifact>,

    #[serde(default)]
    pub links: Option<BundleLinks>,

    #[serde(default)]
    pub signature: Option<BundleSignature>,
}

impl BundleManifest {
    /// Returns the list of service names in this bundle.
    /// For single-service bundles, returns an empty vec (no named services).
    pub fn service_names(&self) -> Vec<&str> {
        match &self.services {
            Some(svcs) if !svcs.is_empty() => svcs.iter().map(|s| s.name.as_str()).collect(),
            _ => vec![],
        }
    }

    /// Returns true if this is a multi-service bundle.
    pub fn is_multi_service(&self) -> bool {
        matches!(&self.services, Some(svcs) if svcs.len() > 1)
    }
}
