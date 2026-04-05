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

/// A WASM artifact within a bundle, abstracting over single-service and
/// multi-service layouts so callers never need to check both shapes.
pub struct WasmArtifact<'a> {
    /// Service name. `None` for single-service bundles.
    pub name: Option<&'a str>,
    pub wasm: &'a BundleArtifact,
    pub abi: Option<&'a BundleArtifact>,
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

    /// Iterate all WASM artifacts uniformly, regardless of single/multi-service.
    ///
    /// - Multi-service (`services` non-empty): yields one `WasmArtifact` per service.
    /// - Single-service: yields one `WasmArtifact` with `name: None` from the
    ///   top-level `wasm`/`abi` fields.
    pub fn wasm_artifacts(&self) -> Vec<WasmArtifact<'_>> {
        match &self.services {
            Some(svcs) if !svcs.is_empty() => svcs
                .iter()
                .map(|s| WasmArtifact {
                    name: Some(&s.name),
                    wasm: &s.wasm,
                    abi: s.abi.as_ref(),
                })
                .collect(),
            _ => self
                .wasm
                .as_ref()
                .map(|w| {
                    vec![WasmArtifact {
                        name: None,
                        wasm: w,
                        abi: self.abi.as_ref(),
                    }]
                })
                .unwrap_or_default(),
        }
    }

    /// Serialize the manifest's display metadata to JSON bytes for storage.
    ///
    /// Extracts `package`, `version`, `metadata.*`, and `links.*` into a flat
    /// JSON object. This replaces the ~70-line inline blocks that were
    /// copy-pasted across the three bundle install paths.
    pub fn to_metadata_json(&self) -> eyre::Result<Vec<u8>> {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "package".into(),
            serde_json::Value::String(self.package.clone()),
        );
        obj.insert(
            "version".into(),
            serde_json::Value::String(self.app_version.clone()),
        );

        if let Some(ref m) = self.metadata {
            obj.insert("name".into(), serde_json::Value::String(m.name.clone()));
            if let Some(ref v) = m.description {
                obj.insert("description".into(), serde_json::Value::String(v.clone()));
            }
            if let Some(ref v) = m.icon {
                obj.insert("icon".into(), serde_json::Value::String(v.clone()));
            }
            if !m.tags.is_empty() {
                obj.insert(
                    "tags".into(),
                    serde_json::Value::Array(
                        m.tags
                            .iter()
                            .map(|t| serde_json::Value::String(t.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(ref v) = m.license {
                obj.insert("license".into(), serde_json::Value::String(v.clone()));
            }
        }

        if let Some(ref l) = self.links {
            let mut links_obj = serde_json::Map::new();
            if let Some(ref v) = l.frontend {
                links_obj.insert("frontend".into(), serde_json::Value::String(v.clone()));
            }
            if let Some(ref v) = l.github {
                links_obj.insert("github".into(), serde_json::Value::String(v.clone()));
            }
            if let Some(ref v) = l.docs {
                links_obj.insert("docs".into(), serde_json::Value::String(v.clone()));
            }
            if !links_obj.is_empty() {
                obj.insert("links".into(), serde_json::Value::Object(links_obj));
            }
        }

        Ok(serde_json::to_vec(&serde_json::Value::Object(obj))?)
    }
}
