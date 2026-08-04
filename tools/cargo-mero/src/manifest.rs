//! `manifest.json` rendering for `cargo mero bundle`: constructs the node's own
//! `BundleManifest` type directly, so a new node-side field is a compile error here.

use calimero_bundle::{
    BundleArtifact, BundleHandlers, BundleLinks, BundleManifest, BundleMetadata, BundleService,
};
use eyre::Result;
use sha2::{Digest, Sha256};

use crate::meta::BundleMeta;

/// Manifest schema version; `"1.0"` is the only value the node accepts.
const MANIFEST_VERSION: &str = "1.0";

/// A staged service: its wasm and abi already copied under the bundle root.
/// `service_name` is `None` for a single-service bundle (top-level `wasm`/`abi`),
/// `Some(name)` for a member of a multi-service `services[]`.
#[derive(Debug, Clone)]
pub struct StagedArtifact {
    pub service_name: Option<String>,
    pub wasm: BundleArtifact,
    pub abi: Option<BundleArtifact>,
}

/// Hashes `bytes` into the manifest's `{path, hash, size}` shape; the node
/// checks artifact bytes against this hash, so a wrong value is uninstallable.
pub fn artifact_from_bytes(rel: impl Into<String>, bytes: &[u8]) -> BundleArtifact {
    BundleArtifact {
        path: rel.into(),
        hash: hex_lower(&Sha256::digest(bytes)),
        size: bytes.len() as u64,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Render `manifest.json`: a single service uses top-level `wasm`/`abi`, anything
/// else emits `services[]`, matching how the node reads them. Signing adds
/// `signerId`/`signature` afterwards.
///
/// No `..` in `BundleManifest::artifacts()`: a node-side field addition must
/// break this build until handled.
pub fn render(meta: &BundleMeta, artifacts: &[StagedArtifact]) -> Result<serde_json::Value> {
    let single = match artifacts {
        [only] if only.service_name.is_none() => Some(only),
        _ => None,
    };
    let (wasm, abi, services) = match single {
        Some(a) => (Some(a.wasm.clone()), a.abi.clone(), None),
        None => {
            let services = artifacts
                .iter()
                .map(|a| BundleService {
                    name: a.service_name.clone().unwrap_or_default(),
                    wasm: a.wasm.clone(),
                    abi: a.abi.clone(),
                })
                .collect();
            (None, None, Some(services))
        }
    };

    let manifest = BundleManifest {
        version: MANIFEST_VERSION.to_owned(),
        package: meta.package.clone(),
        app_version: meta.app_version.clone(),
        min_runtime_version: meta.min_runtime_version.clone(),
        signer_id: None,
        metadata: meta.name.clone().map(|name| BundleMetadata {
            name,
            description: meta.description.clone(),
            author: meta.author.clone(),
            icon: meta.icon.clone(),
            tags: meta.tags.clone(),
            license: meta.license.clone(),
        }),
        // Sibling of `metadata` so it stays outside app-id derivation. Defaults
        // to the package, which is what the deep-link resolver matches on.
        handlers: Some(BundleHandlers {
            slug: Some(meta.slug.clone().unwrap_or_else(|| meta.package.clone())),
        }),
        interfaces: None,
        wasm,
        abi,
        services,
        links: Some(BundleLinks {
            frontend: meta.frontend.clone(),
            github: meta.github.clone(),
            docs: meta.docs.clone(),
        }),
        signature: None,
    };
    Ok(serde_json::to_value(manifest)?)
}

#[cfg(test)]
mod tests {
    use camino::Utf8PathBuf;
    use serde_json::json;

    use super::*;

    fn single_meta() -> BundleMeta {
        BundleMeta {
            package: "com.example.demo".into(),
            name: Some("demo".into()),
            description: Some("A demo app".into()),
            author: Some("Alice".into()),
            icon: None,
            slug: None,
            license: None,
            tags: vec![],
            github: None,
            docs: None,
            min_runtime_version: "0.1.0".into(),
            frontend: None,
            app_version: "1.0.0".into(),
            services: vec![],
            manifest_dir: Utf8PathBuf::new(),
        }
    }

    #[test]
    fn single_service_manifest_shape() {
        let meta = single_meta();
        let artifacts = vec![StagedArtifact {
            service_name: None,
            wasm: artifact_from_bytes("app.wasm", b"wasm-bytes"),
            abi: Some(artifact_from_bytes("abi.json", b"abi-bytes")),
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
                    "author": "Alice",
                    "tags": []
                },
                "handlers": { "slug": "com.example.demo" },
                "wasm": {
                    "path": "app.wasm",
                    "hash": "7db53183cb05feb146262096c5622eb295fe8cdc909dcdcbad8fadb89b6898f7",
                    "size": 10
                },
                "abi": {
                    "path": "abi.json",
                    "hash": "56f2026ee3bf797d070812922ff571bb1b6dbd83965d5f693240c56f47b6700f",
                    "size": 9
                },
                "links": {}
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
            icon: None,
            slug: None,
            license: None,
            tags: vec![],
            github: None,
            docs: None,
            min_runtime_version: "0.0.0".into(),
            frontend: Some("https://example.com".into()),
            app_version: "0.5.0".into(),
            services: vec![],
            manifest_dir: Utf8PathBuf::new(),
        };
        let artifacts = vec![
            StagedArtifact {
                service_name: Some("store-a".into()),
                wasm: artifact_from_bytes("services/store-a.wasm", b"store-a-wasm"),
                abi: Some(artifact_from_bytes(
                    "services/store-a-abi.json",
                    b"store-a-abi",
                )),
            },
            StagedArtifact {
                service_name: Some("store-b".into()),
                wasm: artifact_from_bytes("services/store-b.wasm", b"store-b-wasm"),
                abi: Some(artifact_from_bytes(
                    "services/store-b-abi.json",
                    b"store-b-abi",
                )),
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
                "metadata": {
                    "name": "suite",
                    "tags": []
                },
                "handlers": { "slug": "com.example.suite" },
                "services": [
                    {
                        "name": "store-a",
                        "wasm": {
                            "path": "services/store-a.wasm",
                            "hash": "61e8d9e2e1f7dc925781bb55a64d09a0c4867dda8fdcafb465b3004f5724619d",
                            "size": 12
                        },
                        "abi": {
                            "path": "services/store-a-abi.json",
                            "hash": "5f6b00fd7bd4f7c3c663fd8987eaf1f18da171a7ff33ef89b6870b1af4a381b4",
                            "size": 11
                        }
                    },
                    {
                        "name": "store-b",
                        "wasm": {
                            "path": "services/store-b.wasm",
                            "hash": "4de4533b94fe6acf6dc3e5936c0b4d101cf38fcb5ee8a2e27ff2682e77594a8f",
                            "size": 12
                        },
                        "abi": {
                            "path": "services/store-b-abi.json",
                            "hash": "926a02f053b02970015607052148c4cbdccda06a6c60ca837bec6c3a4790db23",
                            "size": 11
                        }
                    }
                ],
                "links": { "frontend": "https://example.com" }
            })
        );
    }
}
