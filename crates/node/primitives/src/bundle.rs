use serde::{Deserialize, Serialize};

/// Represents an artifact within a bundle (WASM, ABI, migration)
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleArtifact {
    pub path: String,
    pub hash: Option<String>,
    pub size: u64,
}

/// Bundle manifest describing the contents of a bundle archive
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: String,
    pub package: String,
    pub app_version: String,
    pub wasm: Option<BundleArtifact>,
    pub abi: Option<BundleArtifact>,
    pub migrations: Vec<BundleArtifact>,
}
