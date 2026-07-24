//! `manifest.json` rendering for `cargo mero bundle`. The shape is the contract
//! read by the node's bundle deserializer (`calimero-node-primitives`, the
//! `BundleManifest` struct), not the hand-written `build-bundle.sh` heredocs.

use eyre::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::meta::BundleMeta;

/// Manifest schema version; `"1.0"` is the only value the node accepts.
const MANIFEST_VERSION: &str = "1.0";

/// One bundle file entry: its path relative to the bundle root, its byte size,
/// and its content hash.
///
/// `hash` is a lowercase-hex SHA-256 of the file bytes. The node NEVER verifies
/// this per-artifact hash (`BundleArtifact.hash` is an unread `Option<String>`;
/// integrity comes solely from the ed25519 signature over the whole manifest),
/// so we fill a real hash for forward-compat rather than the heredocs' `null`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    pub path: String,
    pub size: u64,
    pub hash: String,
}

impl Artifact {
    pub fn from_bytes(path: impl Into<String>, bytes: &[u8]) -> Self {
        let hash = Sha256::digest(bytes);
        Self {
            path: path.into(),
            size: bytes.len() as u64,
            hash: hex_lower(&hash),
        }
    }
}

/// A staged service: its wasm and abi already copied under the bundle root.
/// `service_name` is `None` for a single-service bundle (top-level `wasm`/`abi`),
/// `Some(name)` for a member of a multi-service `services[]`.
#[derive(Debug, Clone)]
pub struct StagedArtifact {
    pub service_name: Option<String>,
    pub wasm: Artifact,
    pub abi: Artifact,
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The `metadata` block. Core's `BundleMetadata` also carries `icon`/`tags`/
/// `license`, and has no `author` field, but the struct is not
/// `deny_unknown_fields`, so `author` round-trips as a recorded (if node-ignored)
/// field. `name` is required whenever `metadata` is present.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MetadataJson {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    author: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceJson<'a> {
    name: &'a str,
    wasm: &'a Artifact,
    abi: &'a Artifact,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LinksJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    frontend: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ManifestJson<'a> {
    version: &'static str,
    package: &'a str,
    app_version: &'a str,
    min_runtime_version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<MetadataJson>,
    /// Single-service top-level wasm/abi; omitted when `services` is populated.
    #[serde(skip_serializing_if = "Option::is_none")]
    wasm: Option<&'a Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi: Option<&'a Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    services: Option<Vec<ServiceJson<'a>>>,
    migrations: Vec<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    links: Option<LinksJson>,
}

/// Render the `manifest.json` value. A single-service bundle (one artifact with
/// no service name) uses top-level `wasm`/`abi`; anything else emits `services[]`
/// and omits the top-level pair, matching the node's `wasm_artifacts()` dispatch.
/// `signerId`/`signature` are added later by `mero_sign::sign_manifest`.
pub fn render(meta: &BundleMeta, artifacts: &[StagedArtifact]) -> Result<serde_json::Value> {
    let single = match artifacts {
        [only] if only.service_name.is_none() => Some(only),
        _ => None,
    };

    let (wasm, abi, services) = match single {
        Some(a) => (Some(&a.wasm), Some(&a.abi), None),
        None => {
            let services = artifacts
                .iter()
                .map(|a| ServiceJson {
                    name: a.service_name.as_deref().unwrap_or_default(),
                    wasm: &a.wasm,
                    abi: &a.abi,
                })
                .collect();
            (None, None, Some(services))
        }
    };

    let metadata = meta.name.clone().map(|name| MetadataJson {
        name,
        description: meta.description.clone(),
        author: meta.author.clone(),
    });

    let manifest = ManifestJson {
        version: MANIFEST_VERSION,
        package: &meta.package,
        app_version: &meta.app_version,
        min_runtime_version: &meta.min_runtime_version,
        metadata,
        wasm,
        abi,
        services,
        migrations: Vec::new(),
        links: meta
            .frontend
            .clone()
            .map(|f| LinksJson { frontend: Some(f) }),
    };

    Ok(serde_json::to_value(manifest)?)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn single_meta() -> BundleMeta {
        BundleMeta {
            package: "com.example.demo".into(),
            name: Some("demo".into()),
            description: Some("A demo app".into()),
            author: Some("Alice".into()),
            min_runtime_version: "0.1.0".into(),
            frontend: None,
            app_version: "1.0.0".into(),
            services: vec![],
        }
    }

    #[test]
    fn single_service_manifest_shape() {
        let meta = single_meta();
        let artifacts = vec![StagedArtifact {
            service_name: None,
            wasm: Artifact::from_bytes("app.wasm", b"wasm-bytes"),
            abi: Artifact::from_bytes("abi.json", b"abi-bytes"),
        }];

        let manifest = render(&meta, &artifacts).unwrap();

        assert_eq!(
            manifest,
            json!({
                "version": "1.0",
                "package": "com.example.demo",
                "appVersion": "1.0.0",
                "minRuntimeVersion": "0.1.0",
                "metadata": {
                    "name": "demo",
                    "description": "A demo app",
                    "author": "Alice"
                },
                "wasm": {
                    "path": "app.wasm",
                    "size": 10,
                    "hash": "7db53183cb05feb146262096c5622eb295fe8cdc909dcdcbad8fadb89b6898f7"
                },
                "abi": {
                    "path": "abi.json",
                    "size": 9,
                    "hash": "56f2026ee3bf797d070812922ff571bb1b6dbd83965d5f693240c56f47b6700f"
                },
                "migrations": []
            })
        );
    }

    #[test]
    fn multi_service_manifest_shape() {
        let meta = BundleMeta {
            package: "com.example.suite".into(),
            name: Some("suite".into()),
            description: None,
            author: None,
            min_runtime_version: "0.0.0".into(),
            frontend: Some("https://example.com".into()),
            app_version: "0.5.0".into(),
            services: vec![],
        };
        let artifacts = vec![
            StagedArtifact {
                service_name: Some("store-a".into()),
                wasm: Artifact::from_bytes("services/store-a.wasm", b"store-a-wasm"),
                abi: Artifact::from_bytes("services/store-a-abi.json", b"store-a-abi"),
            },
            StagedArtifact {
                service_name: Some("store-b".into()),
                wasm: Artifact::from_bytes("services/store-b.wasm", b"store-b-wasm"),
                abi: Artifact::from_bytes("services/store-b-abi.json", b"store-b-abi"),
            },
        ];

        let manifest = render(&meta, &artifacts).unwrap();

        assert_eq!(
            manifest,
            json!({
                "version": "1.0",
                "package": "com.example.suite",
                "appVersion": "0.5.0",
                "minRuntimeVersion": "0.0.0",
                "metadata": { "name": "suite" },
                "services": [
                    {
                        "name": "store-a",
                        "wasm": {
                            "path": "services/store-a.wasm",
                            "size": 12,
                            "hash": "61e8d9e2e1f7dc925781bb55a64d09a0c4867dda8fdcafb465b3004f5724619d"
                        },
                        "abi": {
                            "path": "services/store-a-abi.json",
                            "size": 11,
                            "hash": "5f6b00fd7bd4f7c3c663fd8987eaf1f18da171a7ff33ef89b6870b1af4a381b4"
                        }
                    },
                    {
                        "name": "store-b",
                        "wasm": {
                            "path": "services/store-b.wasm",
                            "size": 12,
                            "hash": "4de4533b94fe6acf6dc3e5936c0b4d101cf38fcb5ee8a2e27ff2682e77594a8f"
                        },
                        "abi": {
                            "path": "services/store-b-abi.json",
                            "size": 11,
                            "hash": "926a02f053b02970015607052148c4cbdccda06a6c60ca837bec6c3a4790db23"
                        }
                    }
                ],
                "migrations": [],
                "links": { "frontend": "https://example.com" }
            })
        );
    }
}
