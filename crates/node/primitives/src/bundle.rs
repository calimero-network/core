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
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleSignature {
    pub alg: String,
    pub sig: String,
    pub pubkey: String,
    pub signed_at: String,
}

/// Bundle manifest describing the contents of a bundle archive
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: String,
    pub package: String,
    pub app_version: String,

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
