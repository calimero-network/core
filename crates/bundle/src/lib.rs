mod signature;

pub use signature::{
    canonicalize_manifest, compute_bundle_hash, compute_signing_payload, decode_public_key,
    decode_signature, derive_signer_id_did_key, format_bundle_hash, sign_manifest_json,
    verify_ed25519, verify_manifest_signature, ManifestVerification,
};

use serde::{Deserialize, Serialize};

/// The node refuses a bundle's `manifest.json` above this size, before any
/// signature check. `cargo mero` derives its icon budget from this.
pub const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// Represents an artifact within a bundle (WASM, ABI, migration)
///
/// `hash` is what binds the artifact bytes to the manifest signature, so a
/// manifest that omits it is malformed rather than merely unhashed.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleArtifact {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

/// Display metadata for the application
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleMetadata {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frontend: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

/// Deep-link handler declarations for the application.
///
/// Lets an app declare its own deep-link `slug` (e.g. `mero-chat`) so links
/// like `https://links.calimero.network/<slug>/join` and
/// `calimero://<slug>/join` resolve to the right app. When absent, the desktop
/// falls back to kebab-casing the app name.
///
/// This block is a sibling of `metadata` in the manifest, NOT nested inside it,
/// so it is deliberately outside the raw-wasm application-id derivation
/// (`hash(bytecode, size, source, metadata)`).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleHandlers {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
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

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BundleMetadata>,

    /// Deep-link handler declarations. Sibling of `metadata`, so it does not
    /// participate in the raw-wasm application-id derivation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handlers: Option<BundleHandlers>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interfaces: Option<BundleInterfaces>,

    /// Single-service WASM (backward compat). Ignored when `services` is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm: Option<BundleArtifact>,
    /// Single-service ABI (backward compat). Ignored when `services` is non-empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abi: Option<BundleArtifact>,

    /// Named services. When present, each context specifies which service it runs.
    /// If empty/absent, the bundle is single-service and uses `wasm`/`abi` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Vec<BundleService>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub links: Option<BundleLinks>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<BundleSignature>,
}

/// A WASM artifact within a bundle, abstracting over single-service and
/// multi-service layouts so callers never need to check both shapes.
pub struct WasmArtifact<'a> {
    /// Service name. `None` for single-service bundles.
    pub name: Option<&'a str>,
    pub wasm: &'a BundleArtifact,
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
    ///   top-level `wasm` field.
    pub fn wasm_artifacts(&self) -> Vec<WasmArtifact<'_>> {
        match &self.services {
            Some(svcs) if !svcs.is_empty() => svcs
                .iter()
                .map(|s| WasmArtifact {
                    name: Some(&s.name),
                    wasm: &s.wasm,
                })
                .collect(),
            _ => self
                .wasm
                .as_ref()
                .map(|w| {
                    vec![WasmArtifact {
                        name: None,
                        wasm: w,
                    }]
                })
                .unwrap_or_default(),
        }
    }

    /// Every artifact the manifest declares, paired with its manifest field. The
    /// `..`-free destructure stops compiling until a new field is classified.
    pub fn artifacts(&self) -> Vec<(String, &BundleArtifact)> {
        let Self {
            wasm,
            abi,
            services,
            version: _,
            package: _,
            app_version: _,
            signer_id: _,
            min_runtime_version: _,
            metadata: _,
            handlers: _,
            interfaces: _,
            links: _,
            signature: _,
        } = self;

        let mut artifacts = Vec::new();
        if let Some(wasm) = wasm {
            artifacts.push(("wasm".to_owned(), wasm));
        }
        if let Some(abi) = abi {
            artifacts.push(("abi".to_owned(), abi));
        }
        for (i, service) in services.iter().flatten().enumerate() {
            artifacts.push((format!("services[{i}].wasm"), &service.wasm));
            if let Some(abi) = &service.abi {
                artifacts.push((format!("services[{i}].abi"), abi));
            }
        }
        artifacts
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
            if let Some(ref v) = m.author {
                obj.insert("author".into(), serde_json::Value::String(v.clone()));
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

#[cfg(test)]
mod tests {
    use calimero_primitives::application::ApplicationId;
    use calimero_primitives::hash::Hash;

    use super::*;

    /// A minimal manifest JSON with the given optional `handlers` block spliced
    /// in verbatim. Everything else is held constant so the only difference
    /// between two produced manifests is the presence of `handlers`.
    fn manifest_json(handlers_block: &str) -> String {
        format!(
            r#"{{
                "version": "1.0",
                "package": "com.example.deeplink",
                "appVersion": "1.0.0",
                "minRuntimeVersion": "0.1.0",
                "metadata": {{
                    "name": "Mero Chat"
                }},
                {handlers_block}
                "wasm": {{ "path": "app.wasm", "hash": "00", "size": 10 }}
            }}"#
        )
    }

    /// Like `manifest_json`, but splices the given fragment into the
    /// `metadata` object instead of the top-level `handlers` block.
    fn manifest_json_with_metadata(metadata_fragment: &str) -> String {
        format!(
            r#"{{
                "version": "1.0",
                "package": "com.example.deeplink",
                "appVersion": "1.0.0",
                "minRuntimeVersion": "0.1.0",
                "metadata": {{
                    {metadata_fragment}
                    "name": "Mero Chat"
                }},
                "wasm": {{ "path": "app.wasm", "hash": "00", "size": 10 }}
            }}"#
        )
    }

    #[test]
    fn deserializes_handlers_slug() {
        let manifest: BundleManifest =
            serde_json::from_str(&manifest_json(r#""handlers": { "slug": "mero-chat" },"#))
                .expect("manifest with handlers should deserialize");

        let handlers = manifest.handlers.expect("handlers should be present");
        assert_eq!(handlers.slug.as_deref(), Some("mero-chat"));
    }

    #[test]
    fn handlers_defaults_to_none_when_absent() {
        let manifest: BundleManifest = serde_json::from_str(&manifest_json(""))
            .expect("manifest without handlers should deserialize");

        assert!(
            manifest.handlers.is_none(),
            "handlers must default to None when the block is absent"
        );
    }

    #[test]
    fn handlers_absent_block_and_empty_slug_deserialize() {
        // An empty `handlers: {}` block yields Some(handlers) with slug == None
        // thanks to the field-level `#[serde(default)]`.
        let manifest: BundleManifest =
            serde_json::from_str(&manifest_json(r#""handlers": {},"#)).unwrap();
        let handlers = manifest.handlers.expect("empty handlers block present");
        assert!(handlers.slug.is_none());
    }

    /// The deep-link `handlers` field is a sibling of `metadata`, so it must NOT
    /// leak into the display metadata JSON (which is what raw-wasm installs hash
    /// into the application-id). This proves both that `metadata` is unchanged
    /// and that the derived raw-wasm app-id is identical with vs without a
    /// `handlers` block.
    #[test]
    fn handlers_does_not_affect_metadata_or_app_id() {
        let with_handlers: BundleManifest =
            serde_json::from_str(&manifest_json(r#""handlers": { "slug": "mero-chat" },"#))
                .unwrap();
        let without_handlers: BundleManifest = serde_json::from_str(&manifest_json("")).unwrap();

        // 1. The serialized display metadata must be byte-for-byte identical.
        let meta_with = with_handlers.to_metadata_json().unwrap();
        let meta_without = without_handlers.to_metadata_json().unwrap();
        assert_eq!(
            meta_with, meta_without,
            "handlers must not change the display metadata bytes"
        );

        // 2. Re-derive the raw-wasm application-id the same way
        //    `NodeClient::install_raw_wasm` does - hash(bytecode, size, source,
        //    metadata) - and assert it is identical for both manifests.
        let bytecode = Hash::new(b"fake wasm bytecode");
        let size: u64 = 18;
        // `install_raw_wasm` stores the source as a string in `ApplicationMeta`,
        // so hash the string form here.
        let source = "file:///app.wasm".to_owned();

        let derive = |metadata: &[u8]| -> ApplicationId {
            let components = (&bytecode, size, &source, &metadata.to_vec());
            ApplicationId::from(*Hash::hash_borsh(&components).unwrap())
        };

        assert_eq!(
            derive(&meta_with),
            derive(&meta_without),
            "handlers must not change the derived raw-wasm application-id"
        );
    }

    #[test]
    fn author_does_not_shift_the_bundle_app_id() {
        let with: BundleManifest =
            serde_json::from_str(&manifest_json_with_metadata(r#""author": "Acme","#))
                .expect("with author");
        let without: BundleManifest =
            serde_json::from_str(&manifest_json_with_metadata("")).expect("without author");
        // Bundle ids derive from (package, signerId), never from metadata.
        assert_eq!(
            ApplicationId::for_bundle(&with.package, "did:key:z6MkExample").expect("with"),
            ApplicationId::for_bundle(&without.package, "did:key:z6MkExample").expect("without"),
        );
    }
}
