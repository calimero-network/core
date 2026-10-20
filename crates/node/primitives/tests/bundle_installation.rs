//! Tests for bundle installation

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use calimero_node_primitives::bundle::{
    derive_signer_id_did_key, sign_manifest_json, BundleManifest,
};
use calimero_node_primitives::client::application::bundle::MAX_ARCHIVE_BYTES;
use calimero_node_primitives::client::application::lock_application_rows;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{ApplicationId, ApplicationSource};
use calimero_primitives::blobs::BlobId;
use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database, InMemoryDB};
use calimero_store::iter::Iter;
use calimero_store::slice::Slice;
use calimero_store::tx::Transaction;
use calimero_store::Store;
use camino::Utf8PathBuf;
use ed25519_dalek::SigningKey;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::io::Cursor;
use rand::rand_core::UnwrapErr;
use rand::rngs::SysRng;
use sha2::{Digest, Sha256};
use tar::Builder;
use tempfile::TempDir;

mod common;
use common::{create_test_bundle, create_test_node_client, pack_entries, signed_bundle_bytes};

const FRONTEND_INDEX: &[u8] = b"<html><body>hello</body></html>";
const FRONTEND_APP_JS: &[u8] = b"console.log('app');";
const FILES_1_0_0: [(&str, &[u8]); 2] = [
    ("frontend/index.html", FRONTEND_INDEX),
    ("frontend/app.js", FRONTEND_APP_JS),
];
const FILES_2_0_0: [(&str, &[u8]); 2] = [
    ("frontend/index.html", b"<html>v2</html>"),
    ("frontend/app.js", FRONTEND_APP_JS),
];
const FILES_1_5_0: [(&str, &[u8]); 2] = [
    ("frontend/index.html", b"<html>v1.5</html>"),
    ("frontend/app.js", FRONTEND_APP_JS),
];

/// Signs a manifest JSON value and adds the signature field.
/// This is a convenience wrapper around the shared sign_manifest_json function.
fn sign_manifest(manifest_json: &mut serde_json::Value, signing_key: &SigningKey) {
    sign_manifest_json(manifest_json, signing_key).unwrap();
}

/// Lowercase-hex SHA-256, the way `cargo mero bundle` records an artifact hash.
fn artifact_hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[tokio::test]
async fn test_bundle_installation() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a test bundle
    let wasm_content = b"fake wasm bytecode";
    let abi_content = b"{\"types\": []}";
    let migration1: &[u8] = b"CREATE TABLE test (id INT);";
    let migration2: &[u8] = b"CREATE TABLE users (id INT);";
    let migrations: Vec<(&str, &[u8])> = vec![
        ("migrations/001_init.sql", migration1),
        ("migrations/002_add_users.sql", migration2),
    ];

    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.test",
        "1.0.0",
        wasm_content,
        Some(abi_content),
        migrations,
    );

    // Install the bundle
    let application_id = node_client
        .install_application_from_path(bundle_path.clone())
        .await
        .expect("Bundle installation should succeed");

    // Verify application was installed
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist");
    assert!(application.is_some(), "Application should be found");

    // Verify bundle was stored as blob
    let app = application.unwrap();
    let blob_exists = node_client
        .has_blob(&app.blob.bytecode)
        .expect("Should check blob existence");
    assert!(blob_exists, "Bundle blob should exist");

    // The wasm entry must still be picked out of an archive that also carries an
    // ABI and migrations.
    let bytes = node_client
        .get_application_bytes(&application_id, None)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");
    assert_eq!(bytes.as_ref(), wasm_content, "WASM content should match");
}

#[tokio::test]
async fn test_bundle_get_application_bytes() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a test bundle
    let wasm_content = b"fake wasm bytecode for runtime";
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.runtime",
        "2.0.0",
        wasm_content,
        None,
        vec![],
    );

    // Install the bundle
    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");

    // Get application bytes (should read the wasm out of the bundle blob)
    let bytes = node_client
        .get_application_bytes(&application_id, None)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes.as_ref(),
        wasm_content,
        "Application bytes should match WASM content"
    );
}

#[tokio::test]
async fn test_bundle_manifest_validation() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with mismatched package name
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.wrong", // Different package in manifest
        "1.0.0",
        b"wasm",
        None,
        vec![],
    );

    // Install bundle - package/version will be extracted from manifest
    // Since we're extracting from manifest, this test no longer makes sense
    // The package/version will always match because they come from the manifest
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_ok(),
        "Bundle installation should succeed when extracting from manifest"
    );
}

#[tokio::test]
async fn test_bundle_validation_missing_fields() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a bundle with missing package field in manifest
    let bundle_path = temp_dir.path().join("invalid-bundle.mpk");
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // The package key is simply absent. The artifact hash is real, so the only
    // thing this manifest is malformed about is the field the test names.
    let invalid_manifest_json = serde_json::json!({
        "version": "1.0",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10,
            "hash": artifact_hash(b"fake wasm")
        },
        "migrations": []
    });
    let manifest_bytes = serde_json::to_vec(&invalid_manifest_json).unwrap();
    let manifest_json = manifest_bytes.as_slice();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_json.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_json).unwrap();

    // Add a dummy WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(10);
    wasm_header.set_cksum();
    tar.append(&wasm_header, &b"fake wasm"[..]).unwrap();

    tar.finish().unwrap();
    drop(tar); // Ensure tar is dropped and file is flushed
    let bundle_path_utf8: Utf8PathBuf = bundle_path.try_into().unwrap();

    // Installation should fail with validation error
    let result = node_client
        .install_application_from_path(bundle_path_utf8)
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for invalid manifest"
    );
    let error_msg = result.unwrap_err().to_string();
    // Named after the absent field, so nothing looser will do: an empty-package
    // or malformed-archive rejection would mean the manifest never got that far.
    assert!(
        error_msg.contains("missing field `package`"),
        "the absent package field must be what refuses the bundle, got: {error_msg}"
    );
}

/// Create a test bundle with custom WASM path
fn create_test_bundle_custom_wasm_path(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_path: &str,
    wasm_content: &[u8],
) -> Utf8PathBuf {
    let bundle_path = temp_dir.path().join(format!("{package}-{version}.mpk"));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // Generate a signing key for the test bundle
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest.json with custom WASM path (without signature initially)
    let manifest = BundleManifest {
        version: "1.0".to_string(),
        package: package.to_string(),
        app_version: version.to_string(),
        signer_id: Some(signer_id),
        min_runtime_version: "0.1.0".to_string(),
        metadata: None,
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(calimero_node_primitives::bundle::BundleArtifact {
            path: wasm_path.to_string(),
            hash: artifact_hash(wasm_content),
            size: wasm_content.len() as u64,
        }),
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        services: None,
        signature: None,
    };

    // Serialize to JSON value, sign it, then re-serialize
    let mut manifest_json: serde_json::Value = serde_json::to_value(&manifest).unwrap();
    sign_manifest(&mut manifest_json, &signing_key);

    let manifest_bytes = serde_json::to_vec(&manifest_json).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    // Add WASM file at custom path
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path(wasm_path).unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

#[tokio::test]
async fn test_bundle_custom_wasm_path() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with WASM at custom path
    let wasm_content = b"custom path wasm bytecode";
    let bundle_path = create_test_bundle_custom_wasm_path(
        &temp_dir,
        "com.example.custom",
        "1.0.0",
        "src/main.wasm",
        wasm_content,
    );

    // Install the bundle
    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");

    // A custom `wasm.path` must resolve to the entry at that path in the archive
    let bytes = node_client
        .get_application_bytes(&application_id, None)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes.as_ref(),
        wasm_content,
        "Application bytes should match WASM content from custom path"
    );
}

#[tokio::test]
async fn test_bundle_no_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.metadata",
        "1.0.0",
        b"wasm content",
        None,
        vec![],
    );

    // Install bundle (metadata is extracted from manifest in Registry v2)
    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");

    // Verify metadata contains package and version extracted from manifest
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist")
        .expect("Application should be found");

    // Metadata should contain at least package and version
    assert!(
        !application.metadata.is_empty(),
        "Bundle metadata should contain package and version extracted from manifest"
    );

    // Verify package and version are present
    let metadata_json: serde_json::Value =
        serde_json::from_slice(&application.metadata).expect("Metadata should be valid JSON");
    assert_eq!(
        metadata_json["package"], "com.example.metadata",
        "Package should match manifest"
    );
    assert_eq!(
        metadata_json["version"], "1.0.0",
        "Version should match manifest"
    );
}

/// The node stores a bundle's display metadata as opaque bytes and serves them
/// back untouched, so a guide in the manifest must reach `get_application`.
#[tokio::test]
async fn test_bundle_guide_reaches_stored_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let guide = "## Overview\nA test app.\n\n## Procedures\n### Store a value\nCall `set`.\n";
    let wasm_content: &[u8] = b"guide wasm bytecode";
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.guide",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "metadata": { "name": "Guide App", "guide": guide },
        "wasm": {
            "path": "app.wasm",
            "hash": artifact_hash(wasm_content),
            "size": wasm_content.len(),
        },
    });
    sign_manifest(&mut manifest, &SigningKey::generate(&mut UnwrapErr(SysRng)));
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let name = "com.example.guide-1.0.0.mpk";
    let _bundle = pack_entries(
        &temp_dir,
        name,
        &[
            ("manifest.json", manifest_bytes.as_slice()),
            ("app.wasm", wasm_content),
        ],
    );
    let bundle_path: Utf8PathBuf = temp_dir.path().join(name).try_into().unwrap();

    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist")
        .expect("Application should be found");

    let metadata: serde_json::Value =
        serde_json::from_slice(&application.metadata).expect("Metadata should be valid JSON");
    assert_eq!(metadata["guide"], guide);
}

/// A node's sign-in page trusts a login callback to the origins an installed
/// bundle declares, reading them from the stored metadata. An app that moved
/// domain lists its old origin in `frontendAliases`, so the aliases must reach
/// `get_application` next to `links.frontend`.
#[tokio::test]
async fn test_bundle_frontend_aliases_reach_stored_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let wasm_content: &[u8] = b"aliases wasm bytecode";
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.moved",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "metadata": { "name": "Moved App" },
        "links": { "frontend": "https://new.example" },
        "frontendAliases": ["https://old.example", "javascript:alert(1)"],
        "wasm": {
            "path": "app.wasm",
            "hash": artifact_hash(wasm_content),
            "size": wasm_content.len(),
        },
    });
    sign_manifest(&mut manifest, &SigningKey::generate(&mut UnwrapErr(SysRng)));
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let name = "com.example.moved-1.0.0.mpk";
    let _bundle = pack_entries(
        &temp_dir,
        name,
        &[
            ("manifest.json", manifest_bytes.as_slice()),
            ("app.wasm", wasm_content),
        ],
    );
    let bundle_path: Utf8PathBuf = temp_dir.path().join(name).try_into().unwrap();

    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist")
        .expect("Application should be found");

    let metadata: serde_json::Value =
        serde_json::from_slice(&application.metadata).expect("Metadata should be valid JSON");
    assert_eq!(metadata["links"]["frontend"], "https://new.example");
    assert_eq!(
        metadata["frontendAliases"],
        serde_json::json!(["https://old.example"])
    );
}

#[tokio::test]
async fn test_bundle_validation_empty_package() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with empty package field
    let bundle_path = temp_dir.path().join("empty-package.mpk");
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let invalid_manifest_json = serde_json::json!({
        "version": "1.0",
        "package": "",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10,
            "hash": artifact_hash(b"fake wasm")
        },
        "migrations": []
    });
    let manifest_bytes = serde_json::to_vec(&invalid_manifest_json).unwrap();
    let manifest_json = manifest_bytes.as_slice();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_json.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_json).unwrap();

    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(10);
    wasm_header.set_cksum();
    tar.append(&wasm_header, &b"fake wasm"[..]).unwrap();

    tar.finish().unwrap();
    drop(tar);
    let bundle_path_utf8: Utf8PathBuf = bundle_path.try_into().unwrap();

    let result = node_client
        .install_application_from_path(bundle_path_utf8)
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for empty package"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("package") && error_msg.contains("empty"),
        "Error should mention empty package field, got: {error_msg}"
    );
}

#[tokio::test]
async fn test_bundle_validation_empty_app_version() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with empty appVersion field
    let bundle_path = temp_dir.path().join("empty-version.mpk");
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let invalid_manifest_json = serde_json::json!({
        "version": "1.0",
        "package": "com.example.test",
        "appVersion": "",
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10,
            "hash": artifact_hash(b"fake wasm")
        },
        "migrations": []
    });
    let manifest_bytes = serde_json::to_vec(&invalid_manifest_json).unwrap();
    let manifest_json = manifest_bytes.as_slice();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_json.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_json).unwrap();

    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(10);
    wasm_header.set_cksum();
    tar.append(&wasm_header, &b"fake wasm"[..]).unwrap();

    tar.finish().unwrap();
    drop(tar);
    let bundle_path_utf8: Utf8PathBuf = bundle_path.try_into().unwrap();

    let result = node_client
        .install_application_from_path(bundle_path_utf8)
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for empty appVersion"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("appVersion") || error_msg.contains("version"),
        "Error should mention empty appVersion field, got: {error_msg}"
    );
}

#[tokio::test]
async fn test_bundle_validation_missing_app_version() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with missing appVersion field
    let bundle_path = temp_dir.path().join("missing-version.mpk");
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let invalid_manifest_json = serde_json::json!({
        "version": "1.0",
        "package": "com.example.test",
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10,
            "hash": artifact_hash(b"fake wasm")
        },
        "migrations": []
    });
    let manifest_bytes = serde_json::to_vec(&invalid_manifest_json).unwrap();
    let manifest_json = manifest_bytes.as_slice();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_json.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_json).unwrap();

    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(10);
    wasm_header.set_cksum();
    tar.append(&wasm_header, &b"fake wasm"[..]).unwrap();

    tar.finish().unwrap();
    drop(tar);
    let bundle_path_utf8: Utf8PathBuf = bundle_path.try_into().unwrap();

    let result = node_client
        .install_application_from_path(bundle_path_utf8)
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for missing appVersion"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("appVersion")
            || error_msg.contains("missing field")
            || error_msg.contains("version")
            || error_msg.contains("parse"),
        "Error should mention missing appVersion field, got: {error_msg}"
    );
}

#[tokio::test]
async fn test_bundle_package_version_extracted_from_manifest() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with specific package/version
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.extracted",
        "3.7.2",
        b"wasm content",
        None,
        vec![],
    );

    // Install without providing package/version (should extract from manifest)
    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");

    // Verify application metadata has correct package/version from manifest
    let _application = node_client
        .get_application(&application_id)
        .expect("Application should exist")
        .expect("Application should be found");

    // Package/version should be stored in ApplicationMeta (for query functions)
    // We can't directly access ApplicationMeta, but we can verify via list_packages/list_versions
    let packages = node_client.list_packages().expect("Should list packages");
    assert!(
        packages.contains(&"com.example.extracted".to_string()),
        "Package should be listed"
    );

    let versions = node_client
        .list_versions("com.example.extracted")
        .expect("Should list versions");
    assert!(
        versions.contains(&"3.7.2".to_string()),
        "Version should be listed"
    );
}

#[tokio::test]
async fn test_is_bundle_blob() {
    let temp_dir = TempDir::new().unwrap();
    let (_node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a bundle
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.test",
        "1.0.0",
        b"wasm content",
        None,
        vec![],
    );

    // Read bundle bytes
    let bundle_bytes = fs::read(&bundle_path).unwrap();

    // Test bundle detection
    assert!(
        NodeClient::is_bundle_blob(&bundle_bytes),
        "Bundle blob should be detected as bundle"
    );

    // Test non-bundle detection (regular WASM)
    let wasm_bytes = b"fake wasm bytecode";
    assert!(
        !NodeClient::is_bundle_blob(wasm_bytes),
        "Regular WASM should not be detected as bundle"
    );

    // Test non-bundle detection (random bytes)
    let random_bytes = b"random bytes that are not a bundle";
    assert!(
        !NodeClient::is_bundle_blob(random_bytes),
        "Random bytes should not be detected as bundle"
    );
}

#[tokio::test]
async fn test_install_application_from_bundle_blob() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a bundle
    let wasm_content = b"bundle wasm bytecode";
    let abi_content = b"{\"types\": []}";
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.blob",
        "1.0.0",
        wasm_content,
        Some(abi_content),
        vec![],
    );

    // Read bundle and store as blob (simulating blob sharing)
    let bundle_data = fs::read(&bundle_path).unwrap();
    let cursor = Cursor::new(bundle_data.as_slice());
    let (blob_id, _size) = node_client
        .add_blob(cursor, Some(bundle_data.len() as u64), None)
        .await
        .expect("Should add bundle blob");

    // Install application from bundle blob
    let source = "file:///test/bundle.mpk".parse().unwrap();
    let application_id = node_client
        .install_application_from_bundle_blob(&blob_id, &source)
        .await
        .expect("Should install from bundle blob");

    // Verify application was installed
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist");
    assert!(application.is_some(), "Application should be found");

    // Verify we can get application bytes
    let bytes = node_client
        .get_application_bytes(&application_id, None)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes.as_ref(),
        wasm_content,
        "Application bytes should match WASM"
    );
}

#[tokio::test]
async fn test_install_application_from_bundle_blob_no_metadata() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a bundle
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.metadata",
        "1.0.0",
        b"wasm content",
        None,
        vec![],
    );

    // Read bundle and store as blob
    let bundle_data = fs::read(&bundle_path).unwrap();
    let cursor = Cursor::new(bundle_data.as_slice());
    let (blob_id, _size) = node_client
        .add_blob(cursor, Some(bundle_data.len() as u64), None)
        .await
        .expect("Should add bundle blob");

    let source = "file:///test/bundle.mpk".parse().unwrap();
    let application_id = node_client
        .install_application_from_bundle_blob(&blob_id, &source)
        .await
        .expect("Should install from bundle blob without metadata");

    // Verify metadata contains package and version extracted from manifest
    // (Registry v2: metadata is extracted from bundle manifest)
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist")
        .expect("Application should be found");

    // Metadata should contain at least package and version
    assert!(
        !application.metadata.is_empty(),
        "Bundle metadata should contain package and version extracted from manifest"
    );

    // Verify package and version are present
    let metadata_json: serde_json::Value =
        serde_json::from_slice(&application.metadata).expect("Metadata should be valid JSON");
    assert_eq!(
        metadata_json["package"], "com.example.metadata",
        "Package should match manifest"
    );
    assert_eq!(
        metadata_json["version"], "1.0.0",
        "Version should match manifest"
    );
}

#[tokio::test]
async fn test_install_application_from_bundle_blob_missing_blob() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a non-existent blob ID
    use calimero_primitives::blobs::BlobId;
    let fake_blob_id = BlobId::from([1; 32]);

    let source = "file:///test/bundle.mpk".parse().unwrap();
    let result = node_client
        .install_application_from_bundle_blob(&fake_blob_id, &source)
        .await;

    assert!(result.is_err(), "Should fail when blob doesn't exist");
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("not found") || error_msg.contains("fatal"),
        "Error should mention blob not found, got: {error_msg}"
    );
}

/// Integration test simulating the bundle blob sharing flow:
/// User 1 installs bundle → User 2 receives blob → User 2 installs automatically
///
/// This test verifies that when a bundle blob is shared between nodes,
/// the receiving node can correctly detect and install it, maintaining
/// ApplicationId consistency across nodes.
///
/// NOTE: This test simulates the blob sharing part of the invitation flow.
/// The full invitation flow would require:
/// 1. User 1 installs bundle
/// 2. User 1 creates context with bundle application
/// 3. User 1 invites User 2 (stores context config on external chain/contract)
/// 4. User 2 receives invitation
/// 5. User 2 calls sync_context_config (which fetches context config from external)
/// 6. sync_context_config detects bundle blob exists locally
/// 7. sync_context_config calls install_expected_bundle_blob
/// 8. User 2 can now use the context
///
/// To test the full flow, we would need to:
/// - Set up ContextClient instances (requires external client mocking)
/// - Mock external config client methods (application(), application_revision(), members_revision(), members())
/// - Simulate context creation and invitation
///
/// The current test verifies the critical integration point: when a bundle blob
/// exists locally, install_expected_bundle_blob correctly installs it
/// with the same ApplicationId as the original installation.
#[tokio::test]
async fn test_bundle_blob_sharing_integration() {
    let temp_dir = TempDir::new().unwrap();

    // Create User 1's node client
    let (node_client_1, _data_dir_1, _blob_dir_1) = create_test_node_client(None).await;

    // Create User 2's node client (separate instance)
    let (node_client_2, _data_dir_2, blob_dir_2) = create_test_node_client(None).await;

    // Step 1: User 1 installs bundle
    let wasm_content = b"integration test wasm bytecode";
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.integration",
        "1.0.0",
        wasm_content,
        None,
        vec![],
    );

    let application_id_user1 = node_client_1
        .install_application_from_path(bundle_path.clone())
        .await
        .expect("User 1 should install bundle successfully");

    // Verify User 1 has the application
    let app_user1 = node_client_1
        .get_application(&application_id_user1)
        .expect("Application should exist")
        .expect("Application should be found");

    let bundle_blob_id = app_user1.blob.bytecode;
    let bundle_size = app_user1.size;
    let bundle_source = app_user1.source;

    // Step 2: User 2 receives the bundle blob (simulating blob sharing)
    // Read bundle from User 1's blobstore (bundle file was deleted after installation)
    let bundle_data = node_client_1
        .get_blob_bytes(&bundle_blob_id, None)
        .await
        .expect("Should get bundle blob from User 1's blobstore")
        .expect("Bundle blob should exist");
    let cursor = Cursor::new(bundle_data.as_ref());
    let (received_blob_id, received_size) = node_client_2
        .add_blob(cursor, Some(bundle_data.len() as u64), None)
        .await
        .expect("User 2 should receive bundle blob");

    assert_eq!(
        received_blob_id, bundle_blob_id,
        "Received blob ID should match original bundle blob ID"
    );
    assert_eq!(
        received_size, bundle_size,
        "Received blob size should match original bundle size"
    );

    // Step 3: User 2 doesn't have the application yet
    assert!(
        node_client_2
            .get_application(&application_id_user1)
            .unwrap()
            .is_none(),
        "User 2 should not have application before sync"
    );

    // Step 4: Simulate sync_context_config by manually installing from blob
    // This simulates what happens when sync_context_config detects the blob
    // No metadata needed - bundle detection happens via is_bundle_blob()
    let application_id_user2 = node_client_2
        .install_expected_bundle_blob(
            &application_id_user1,
            &bundle_blob_id,
            &bundle_source, // Use same source as User 1
        )
        .await
        .expect("User 2 should install from bundle blob");

    // Step 5: Verify ApplicationId consistency
    // Same blob_id + size + source + same metadata (extracted from manifest) = same ApplicationId
    assert_eq!(
        application_id_user1, application_id_user2,
        "ApplicationId should be identical (same blob_id, size, source, and metadata from manifest)"
    );

    // Verify both can access their applications
    let app_user1_final = node_client_1
        .get_application(&application_id_user1)
        .expect("Application should exist")
        .expect("Application should be found");

    let app_user2_final = node_client_2
        .get_application(&application_id_user2)
        .expect("Application should exist")
        .expect("Application should be found");

    // Critical: All fields should match (blob ID, size, source, metadata)
    assert_eq!(
        app_user1_final.blob.bytecode, app_user2_final.blob.bytecode,
        "Blob IDs should be identical (same bundle content)"
    );
    assert_eq!(
        app_user1_final.size, app_user2_final.size,
        "Sizes should be identical"
    );
    assert_eq!(
        app_user1_final.source.to_string(),
        app_user2_final.source.to_string(),
        "Sources should be identical"
    );
    assert_eq!(
        app_user1_final.metadata, app_user2_final.metadata,
        "Metadata should be identical (extracted from same bundle manifest)"
    );

    // Both should have metadata extracted from bundle manifest (Registry v2)
    assert!(
        !app_user1_final.metadata.is_empty(),
        "User 1 should have metadata extracted from bundle manifest"
    );
    assert!(
        !app_user2_final.metadata.is_empty(),
        "User 2 should have metadata extracted from bundle manifest"
    );

    // Verify metadata contains package and version
    let metadata_json: serde_json::Value =
        serde_json::from_slice(&app_user1_final.metadata).expect("Metadata should be valid JSON");
    assert_eq!(
        metadata_json["package"], "com.example.integration",
        "Package should match manifest"
    );
    assert_eq!(
        metadata_json["version"], "1.0.0",
        "Version should match manifest"
    );

    // Step 6: Verify User 2 can get application bytes (WASM)
    let bytes_user2 = node_client_2
        .get_application_bytes(&application_id_user2, None)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes_user2.as_ref(),
        wasm_content,
        "User 2 should be able to read WASM from bundle"
    );

    // Step 7: The received bundle stays a blob on User 2's node
    let applications_2 = blob_dir_2.path().join("applications");
    assert!(
        !applications_2.exists(),
        "no artifacts should be unpacked to disk, found {}",
        applications_2.display()
    );
}

// Note: install_application_from_url tests require HTTP/HTTPS URLs and would need
// a mock HTTP server or real server. The URL installation logic is covered by
// integration tests. Path-based installation (which covers the same code paths
// for bundle detection) is tested below.

// Signature Verification Integration Tests

/// Helper to create an unsigned bundle (no signature field in manifest)
fn create_unsigned_bundle(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_content: &[u8],
) -> Utf8PathBuf {
    let bundle_path = temp_dir
        .path()
        .join(format!("{package}-{version}-unsigned.mpk"));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // Create manifest.json without signature
    let manifest_json = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": wasm_content.len(),
            "hash": artifact_hash(wasm_content)
        },
        "migrations": []
        // Note: no "signature" field
    });

    let manifest_bytes = serde_json::to_vec(&manifest_json).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    // Add WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// Helper to create a bundle with tampered content (signature was valid at signing,
/// but manifest was modified after signing)
fn create_tampered_bundle(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_content: &[u8],
) -> Utf8PathBuf {
    let bundle_path = temp_dir
        .path()
        .join(format!("{package}-{version}-tampered.mpk"));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // Generate a signing key
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create the ORIGINAL manifest (what was signed)
    let mut original_manifest = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": wasm_content.len(),
            "hash": artifact_hash(wasm_content)
        },
        "migrations": []
    });

    // Sign the original manifest
    sign_manifest(&mut original_manifest, &signing_key);

    // Extract the signature we just created
    let signature = original_manifest.get("signature").unwrap().clone();

    // Create a TAMPERED manifest with different appVersion
    let tampered_manifest = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": "999.999.999",  // TAMPERED!
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": wasm_content.len(),
            "hash": artifact_hash(wasm_content)
        },
        "migrations": [],
        "signature": signature  // Original signature (won't match tampered content)
    });

    let manifest_bytes = serde_json::to_vec(&tampered_manifest).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    // Add WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// Helper to create a bundle where manifest.signerId doesn't match the signing key.
///
/// This manually constructs the mismatch by:
/// 1. Creating a manifest with signerId from key B
/// 2. Signing it with key A (without using sign_manifest_json which would overwrite signerId)
/// 3. The result has signerId B but signature from key A
fn create_signer_id_mismatch_bundle(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_content: &[u8],
) -> Utf8PathBuf {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use calimero_node_primitives::bundle::{canonicalize_manifest, compute_signing_payload};
    use ed25519_dalek::Signer;

    let bundle_path = temp_dir
        .path()
        .join(format!("{package}-{version}-signer-mismatch.mpk"));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // Generate signing key A (will be used for signing)
    let signing_key_a = SigningKey::generate(&mut UnwrapErr(SysRng));

    // Generate signing key B (its signerId will be declared in manifest)
    let signing_key_b = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id_b = derive_signer_id_did_key(signing_key_b.verifying_key().as_bytes());

    // Create manifest that claims signerId from key B
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": signer_id_b,  // Claims to be signed by key B
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": wasm_content.len(),
            "hash": artifact_hash(wasm_content)
        },
        "migrations": []
    });

    // Manually sign with key A without overwriting signerId (to create mismatch)
    let canonical_bytes = canonicalize_manifest(&manifest).unwrap();
    let signing_payload = compute_signing_payload(&canonical_bytes);
    let signature = signing_key_a.sign(&signing_payload);

    // Add signature from key A (but signerId is still from key B - mismatch!)
    let public_key_b64 = URL_SAFE_NO_PAD.encode(signing_key_a.verifying_key().as_bytes());
    let signature_b64 = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    manifest.as_object_mut().unwrap().insert(
        "signature".to_string(),
        serde_json::json!({
            "algorithm": "ed25519",
            "publicKey": public_key_b64,
            "signature": signature_b64
        }),
    );

    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    // Add WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// Helper to create a test bundle with a specific signing key (for ApplicationId derivation tests)
fn create_test_bundle_with_key(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_content: &[u8],
    signing_key: &SigningKey,
) -> Utf8PathBuf {
    let bundle_path = temp_dir
        .path()
        .join(format!("{package}-{version}-keyed.mpk"));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest.json
    let mut manifest_json: serde_json::Value = serde_json::to_value(&BundleManifest {
        version: "1.0".to_string(),
        package: package.to_string(),
        app_version: version.to_string(),
        signer_id: Some(signer_id),
        min_runtime_version: "0.1.0".to_string(),
        metadata: None,
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(calimero_node_primitives::bundle::BundleArtifact {
            path: "app.wasm".to_string(),
            hash: artifact_hash(wasm_content),
            size: wasm_content.len() as u64,
        }),
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        services: None,
        signature: None,
    })
    .unwrap();

    // Sign with the provided key
    sign_manifest(&mut manifest_json, signing_key);

    let manifest_bytes = serde_json::to_vec(&manifest_json).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    // Add WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// An unsigned bundle is not installable by any path. `create_unsigned_bundle`
/// is kept because this is the only test that needs one.
#[tokio::test]
async fn test_unsigned_bundle_installation_is_rejected() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let bundle_path =
        create_unsigned_bundle(&temp_dir, "com.example.unsigned", "1.0.0", b"wasm content");

    let err = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect_err("an unsigned bundle must not install");
    assert!(
        err.to_string().contains("signature"),
        "error should name the missing signature, got: {err}"
    );
}

/// Test: Installation should fail when bundle has been tampered after signing.
///
/// If the manifest content is modified after signing (e.g., appVersion changed),
/// the signature verification MUST fail and installation MUST be rejected.
#[tokio::test]
async fn test_bundle_installation_fails_with_invalid_signature() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a tampered bundle (signed manifest was modified after signing)
    let bundle_path =
        create_tampered_bundle(&temp_dir, "com.example.tampered", "1.0.0", b"wasm content");

    // Installation should fail due to invalid signature
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with tampered content"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("signature") || error_msg.contains("verification"),
        "Error should mention signature verification failure, got: {error_msg}"
    );
}

/// Test: Installation should fail when manifest.signerId doesn't match the signing key.
///
/// The signerId in the manifest MUST match the signerId derived from the
/// signature.publicKey. A mismatch indicates the manifest claims a different
/// signing authority than who actually signed it.
#[tokio::test]
async fn test_bundle_installation_fails_signer_id_mismatch() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create a bundle where signerId doesn't match the signing key
    let bundle_path = create_signer_id_mismatch_bundle(
        &temp_dir,
        "com.example.mismatch",
        "1.0.0",
        b"wasm content",
    );

    // Installation should fail
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with signerId mismatch"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("signerId") || error_msg.contains("mismatch"),
        "Error should mention signerId mismatch, got: {error_msg}"
    );
}

/// Test: ApplicationId should be derived from (package, signer_id).
///
/// This verifies that:
/// - Same package + different signers = different ApplicationIds
/// - Same package + same signer + different versions = same ApplicationId
///
/// This ensures cryptographic update authority is enforced: only bundles
/// signed by the same signerId can update the same application.
#[tokio::test]
async fn test_bundle_application_id_derived_from_package_and_signer_id() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Generate two different signing keys
    let signing_key_s1 = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signing_key_s2 = SigningKey::generate(&mut UnwrapErr(SysRng));

    let signer_id_s1 = derive_signer_id_did_key(signing_key_s1.verifying_key().as_bytes());
    let signer_id_s2 = derive_signer_id_did_key(signing_key_s2.verifying_key().as_bytes());

    // Verify the two signerIds are different
    assert_ne!(
        signer_id_s1, signer_id_s2,
        "Test setup: signerIds should be different"
    );

    let package = "com.example.signer-test";

    // Bundle A: package "com.example.signer-test", signer S1, version "1.0.0"
    let bundle_a = create_test_bundle_with_key(
        &temp_dir,
        package,
        "1.0.0",
        b"wasm v1 signer1",
        &signing_key_s1,
    );

    let app_id_a = node_client
        .install_application_from_path(bundle_a)
        .await
        .expect("Bundle A installation should succeed");

    // Bundle B: package "com.example.signer-test", signer S2, version "1.0.0"
    // Same package, same version, but DIFFERENT signer
    let bundle_b = create_test_bundle_with_key(
        &temp_dir,
        package,
        "1.0.0-s2", // Slightly different version to avoid storage conflicts
        b"wasm v1 signer2",
        &signing_key_s2,
    );

    let app_id_b = node_client
        .install_application_from_path(bundle_b)
        .await
        .expect("Bundle B installation should succeed");

    // Bundle C: package "com.example.signer-test", signer S1, version "2.0.0"
    // Same package, same signer as A, but DIFFERENT version
    let bundle_c = create_test_bundle_with_key(
        &temp_dir,
        package,
        "2.0.0",
        b"wasm v2 signer1",
        &signing_key_s1,
    );

    let app_id_c = node_client
        .install_application_from_path(bundle_c)
        .await
        .expect("Bundle C installation should succeed");

    // ASSERTION 1: app_id_a != app_id_b (different signers = different ApplicationIds)
    // This ensures that applications signed by different authorities are treated
    // as completely different applications, even with the same package name.
    assert_ne!(
        app_id_a, app_id_b,
        "Same package with different signers should produce different ApplicationIds.\n\
         app_id_a (signer S1): {app_id_a}\n\
         app_id_b (signer S2): {app_id_b}"
    );

    // ASSERTION 2: app_id_a == app_id_c (same package + signer = same ApplicationId)
    // Version upgrades for the same BytecodeId should have the same ApplicationId.
    assert_eq!(
        app_id_a, app_id_c,
        "Same package with same signer should produce same ApplicationId (version upgrade).\n\
         app_id_a (v1.0.0): {app_id_a}\n\
         app_id_c (v2.0.0): {app_id_c}"
    );

    // Verify all applications can be retrieved
    assert!(
        node_client.get_application(&app_id_a).unwrap().is_some(),
        "Application A should exist"
    );
    assert!(
        node_client.get_application(&app_id_b).unwrap().is_some(),
        "Application B should exist"
    );
    // Note: app_id_c == app_id_a, so it's the same application (version upgrade)

    // Verify the metadata shows the correct package information
    let app_a = node_client
        .get_application(&app_id_a)
        .expect("Should get application A")
        .expect("Application A should exist");

    let app_b = node_client
        .get_application(&app_id_b)
        .expect("Should get application B")
        .expect("Application B should exist");

    let metadata_a: serde_json::Value =
        serde_json::from_slice(&app_a.metadata).expect("Metadata A should be valid JSON");
    let metadata_b: serde_json::Value =
        serde_json::from_slice(&app_b.metadata).expect("Metadata B should be valid JSON");

    // Both should have the same package name
    assert_eq!(
        metadata_a["package"], package,
        "Application A should have correct package"
    );
    assert_eq!(
        metadata_b["package"], package,
        "Application B should have correct package"
    );

    // Note: signerId is stored in ApplicationMeta.signer_id field in the database,
    // not in the metadata JSON blob. The ApplicationId derivation using (package, signer_id)
    // is verified by the assertions above (different signerIds produce different ApplicationIds).
}

/// The blob is the only copy of a bundle's bytes: nothing is unpacked to disk,
/// and the wasm is still served.
#[tokio::test]
async fn test_bundle_get_application_bytes_needs_no_disk_copy() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

    // Create a test bundle
    let wasm_content = b"blob-only test wasm bytecode";
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.blobonly",
        "1.0.0",
        wasm_content,
        None,
        vec![],
    );

    // Install the bundle
    let application_id = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect("Bundle installation should succeed");

    let bytes = node_client
        .get_application_bytes(&application_id, None)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes.as_ref(),
        wasm_content,
        "Application bytes should match WASM content (served from the bundle blob)"
    );

    let applications = blob_dir.path().join("applications");
    assert!(
        !applications.exists(),
        "no artifacts should be unpacked to disk, found {}",
        applications.display()
    );
}

#[tokio::test]
async fn test_get_latest_version_semantic_ordering() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let package = "com.example.versioning";

    // Install multiple versions in non-sequential order
    // This tests that semantic version comparison works correctly
    let versions = vec!["1.0.0", "2.0.0", "10.0.0", "1.5.0", "1.10.0", "2.5.0"];

    let mut application_ids = Vec::new();
    for version in &versions {
        let bundle_path =
            create_test_bundle(&temp_dir, package, version, b"wasm content", None, vec![]);

        let app_id = node_client
            .install_application_from_path(bundle_path)
            .await
            .expect("Bundle installation should succeed");

        application_ids.push((version.to_string(), app_id));
    }

    // Get latest version - should be "10.0.0" (not "2.5.0" which would be lexicographically latest)
    let (latest_version_str, _latest_app_id) = node_client
        .get_latest_version(package)
        .expect("Should get latest version")
        .expect("Latest version should exist");

    assert_eq!(
        latest_version_str, "10.0.0",
        "Latest version should be 10.0.0 (semantic), not 2.5.0 (lexicographic)"
    );
}

#[tokio::test]
async fn test_get_latest_version_mixed_semver_and_non_semver() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let package = "com.example.mixed";

    // Install mix of semantic versions and non-semantic versions
    let versions = vec!["1.0.0", "invalid-version", "2.0.0", "also-invalid"];

    let mut application_ids = Vec::new();
    for version in &versions {
        let bundle_path =
            create_test_bundle(&temp_dir, package, version, b"wasm content", None, vec![]);

        let app_id = node_client
            .install_application_from_path(bundle_path)
            .await
            .expect("Bundle installation should succeed");

        application_ids.push((version.to_string(), app_id));
    }

    // Get latest version - should prefer semantic versions over non-semantic
    let (latest_version_str, _latest_app_id) = node_client
        .get_latest_version(package)
        .expect("Should get latest version")
        .expect("Latest version should exist");

    // Should be "2.0.0" (latest semantic version), not "invalid-version" or "also-invalid"
    assert_eq!(
        latest_version_str, "2.0.0",
        "Latest version should be 2.0.0 (semantic), preferring semantic versions over non-semantic"
    );
}

// Path Traversal Validation Tests

/// Helper to create a bundle with a custom manifest (for testing malicious inputs)
fn create_bundle_with_custom_manifest(
    temp_dir: &TempDir,
    manifest_json: serde_json::Value,
    wasm_content: &[u8],
) -> Utf8PathBuf {
    let bundle_path = temp_dir.path().join("malicious-bundle.mpk");
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let manifest_bytes = serde_json::to_vec(&manifest_json).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_bytes.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_bytes.as_slice())
        .unwrap();

    // Add WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// Test: Installation should fail when package contains path traversal sequence ".."
///
/// A malicious bundle could set package to "../../../etc" to escape the
/// applications directory and write files to arbitrary locations.
#[tokio::test]
async fn test_bundle_validation_path_traversal_in_package() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Generate a signing key
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest with path traversal in package
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "../../../etc/malicious",  // Path traversal!
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 12,
            "hash": artifact_hash(b"wasm content")
        },
        "migrations": []
    });

    // Sign the manifest
    sign_manifest(&mut manifest, &signing_key);

    let bundle_path = create_bundle_with_custom_manifest(&temp_dir, manifest, b"wasm content");

    // Installation should fail due to path traversal
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with path traversal in package"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("path traversal") || error_msg.contains("'..'"),
        "Error should mention path traversal, got: {error_msg}"
    );
}

/// Test: Installation should fail when appVersion contains path traversal sequence ".."
#[tokio::test]
async fn test_bundle_validation_path_traversal_in_version() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Generate a signing key
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest with path traversal in appVersion
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.test",
        "appVersion": "../../../tmp/malicious",  // Path traversal!
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 12,
            "hash": artifact_hash(b"wasm content")
        },
        "migrations": []
    });

    // Sign the manifest
    sign_manifest(&mut manifest, &signing_key);

    let bundle_path = create_bundle_with_custom_manifest(&temp_dir, manifest, b"wasm content");

    // Installation should fail due to path traversal
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with path traversal in appVersion"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("path traversal") || error_msg.contains("'..'"),
        "Error should mention path traversal, got: {error_msg}"
    );
}

/// Test: a service's wasm path is validated like every other artifact path.
///
/// The single- and multi-service shapes must not diverge here: a validator that
/// covers `wasm.path` but skips `services[].wasm.path` is a trap for whoever
/// next resolves a manifest path against a path on disk.
#[tokio::test]
async fn test_bundle_validation_path_traversal_in_service_wasm_path() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.test",
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "services": [{
            "name": "svc",
            "wasm": {
                "path": "../../../etc/malicious.wasm",  // Path traversal!
                "size": 12,
                "hash": artifact_hash(b"wasm content")
            }
        }],
        "migrations": []
    });

    sign_manifest(&mut manifest, &signing_key);

    let bundle_path = create_bundle_with_custom_manifest(&temp_dir, manifest, b"wasm content");

    let err = node_client
        .install_application_from_path(bundle_path)
        .await
        .expect_err("a service wasm path with '..' must not install");

    let error_msg = err.to_string();
    assert!(
        error_msg.contains("services[0].wasm.path") && error_msg.contains("'..'"),
        "Error should name the offending service field, got: {error_msg}"
    );
}

/// Test: Installation should fail when package contains forward slash
#[tokio::test]
async fn test_bundle_validation_forward_slash_in_package() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Generate a signing key
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest with forward slash in package
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "foo/bar",  // Directory separator!
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 12,
            "hash": artifact_hash(b"wasm content")
        },
        "migrations": []
    });

    // Sign the manifest
    sign_manifest(&mut manifest, &signing_key);

    let bundle_path = create_bundle_with_custom_manifest(&temp_dir, manifest, b"wasm content");

    // Installation should fail due to directory separator
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with forward slash in package"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("directory separator") || error_msg.contains("package"),
        "Error should mention directory separator, got: {error_msg}"
    );
}

/// Test: Installation should fail when package contains backslash (Windows path separator)
#[tokio::test]
async fn test_bundle_validation_backslash_in_package() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Generate a signing key
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest with backslash in package
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "foo\\bar",  // Windows directory separator!
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 12,
            "hash": artifact_hash(b"wasm content")
        },
        "migrations": []
    });

    // Sign the manifest
    sign_manifest(&mut manifest, &signing_key);

    let bundle_path = create_bundle_with_custom_manifest(&temp_dir, manifest, b"wasm content");

    // Installation should fail due to directory separator
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with backslash in package"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("directory separator") || error_msg.contains("package"),
        "Error should mention directory separator, got: {error_msg}"
    );
}

/// Test: Installation should fail when package looks like a Windows absolute path
#[tokio::test]
async fn test_bundle_validation_windows_absolute_path_in_package() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Generate a signing key
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let signer_id = derive_signer_id_did_key(signing_key.verifying_key().as_bytes());

    // Create manifest with Windows absolute path in package
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "C:malicious",  // Windows absolute path!
        "appVersion": "1.0.0",
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": 12,
            "hash": artifact_hash(b"wasm content")
        },
        "migrations": []
    });

    // Sign the manifest
    sign_manifest(&mut manifest, &signing_key);

    let bundle_path = create_bundle_with_custom_manifest(&temp_dir, manifest, b"wasm content");

    // Installation should fail due to absolute path
    let result = node_client.install_application_from_path(bundle_path).await;

    assert!(
        result.is_err(),
        "Bundle installation should fail with Windows absolute path in package"
    );

    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("absolute path") || error_msg.contains("package"),
        "Error should mention absolute path, got: {error_msg}"
    );
}

/// Test: Valid package names should be accepted
#[tokio::test]
async fn test_bundle_validation_valid_package_names() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Test various valid package name formats
    let valid_packages = [
        "com.example.myapp",
        "my-app",
        "my_app_v2",
        "MyApp",
        "app123",
        "com.calimero.kv-store",
    ];

    for (i, package) in valid_packages.iter().enumerate() {
        let bundle_path = create_test_bundle(
            &temp_dir,
            package,
            &format!("1.0.{i}"), // Unique version to avoid conflicts
            b"wasm content",
            None,
            vec![],
        );

        let result = node_client.install_application_from_path(bundle_path).await;

        assert!(
            result.is_ok(),
            "Bundle with valid package '{}' should install successfully, got error: {:?}",
            package,
            result.err()
        );
    }
}

/// Raw wasm is not an application: only a signed `.mpk` carries a re-derivable
/// id. The filename is not the check, so both spellings must be refused.
#[tokio::test]
async fn a_raw_wasm_payload_is_refused() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    for name in ["app.wasm", "app.mpk"] {
        let path = temp_dir.path().join(name);
        fs::write(&path, b"\0asm\x01\0\0\0").unwrap();
        let path: Utf8PathBuf = path.try_into().unwrap();

        let err = node_client
            .install_application_from_path(path)
            .await
            .expect_err("raw wasm must not install");
        assert!(
            err.to_string().contains("not a signed application bundle"),
            "{name} got: {err}"
        );
    }
}

/// A file over the bundle cap must be refused by its metadata length, before
/// it is ever read into memory. `set_len` makes a sparse file so the test
/// does not actually write out the cap's worth of bytes.
#[tokio::test]
async fn install_from_path_rejects_a_file_over_the_bundle_cap() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    let path = temp_dir.path().join("huge.mpk");
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_ARCHIVE_BYTES + 1).unwrap();
    let path: Utf8PathBuf = path.try_into().unwrap();

    let err = node_client
        .install_application_from_path(path)
        .await
        .expect_err("a file over the bundle cap must be refused");
    assert!(
        err.to_string().contains(&MAX_ARCHIVE_BYTES.to_string()),
        "error should name the byte cap, got: {err}"
    );
}

async fn install_bundle_bytes(node_client: &NodeClient, bytes: Vec<u8>) -> ApplicationId {
    try_install(node_client, bytes)
        .await
        .expect("install the bundle")
}

/// mero-mcp and meroctl read only the list, so each entry must carry its
/// services exactly as the single-application read does.
#[tokio::test]
async fn list_applications_reports_each_applications_services() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let (bytes, multi_id) =
        signed_bundle_bytes("com.example.multi", "1.0.0", &["docs", "registry"]);
    install_bundle_bytes(&node_client, bytes).await;

    let listed = node_client
        .list_applications()
        .expect("list applications")
        .into_iter()
        .find(|app| app.id == multi_id)
        .expect("the installed application is listed");
    let read = node_client
        .get_application(&multi_id)
        .expect("read the application")
        .expect("the application exists");

    assert_eq!(
        listed.services.keys().collect::<Vec<_>>(),
        ["docs", "registry"]
    );
    assert_eq!(listed.services, read.services);
}

#[tokio::test]
async fn list_applications_leaves_a_single_service_application_without_services() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let (bytes, single_id) = signed_bundle_bytes("com.example.single", "1.0.0", &[]);
    install_bundle_bytes(&node_client, bytes).await;

    let listed = node_client
        .list_applications()
        .expect("list applications")
        .into_iter()
        .find(|app| app.id == single_id)
        .expect("the installed application is listed");

    assert!(listed.services.is_empty());
}

#[tokio::test]
async fn a_service_blob_counts_as_an_application_artifact() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let (bytes, id) = signed_bundle_bytes("com.example.multi", "1.0.0", &["docs", "registry"]);
    install_bundle_bytes(&node_client, bytes).await;

    let app = node_client
        .get_application(&id)
        .expect("read the application")
        .expect("the application exists");

    for service in app.services.values() {
        assert!(node_client
            .is_blob_application_artifact(&service.bytecode)
            .expect("artifact check"));
    }
}

/// A signed single-service bundle whose manifest lists `listed` as its frontend
/// (entry `frontend/index.html`) while the archive holds `shipped`.
fn frontend_bundle(listed: &[(&str, &[u8])], shipped: &[(&str, &[u8])]) -> Vec<u8> {
    let signing_key = SigningKey::generate(&mut UnwrapErr(SysRng));
    frontend_bundle_by(&signing_key, "1.0.0", listed, shipped)
}

/// As [`frontend_bundle`], under a caller-held key, so releases share an id.
fn frontend_bundle_by(
    signing_key: &SigningKey,
    version: &str,
    listed: &[(&str, &[u8])],
    shipped: &[(&str, &[u8])],
) -> Vec<u8> {
    signed_frontend_bundle(signing_key, version, listed, shipped, None)
}

fn signed_frontend_bundle(
    signing_key: &SigningKey,
    version: &str,
    listed: &[(&str, &[u8])],
    shipped: &[(&str, &[u8])],
    wasm_size: Option<usize>,
) -> Vec<u8> {
    let dir = TempDir::new().unwrap();
    let wasm: &[u8] = b"frontend test wasm";
    let artifact = |path: &str, bytes: &[u8]| serde_json::json!({ "path": path, "hash": artifact_hash(bytes), "size": bytes.len() });
    let files: Vec<_> = listed.iter().map(|(p, b)| artifact(p, b)).collect();
    let mut wasm_artifact = artifact("svc.wasm", wasm);
    if let Some(size) = wasm_size {
        wasm_artifact["size"] = size.into();
    }
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": "com.example.frontend",
        "appVersion": version,
        "signerId": derive_signer_id_did_key(signing_key.verifying_key().as_bytes()),
        "minRuntimeVersion": "0.1.0",
        "services": [{ "name": "svc", "wasm": wasm_artifact }],
        "frontend": { "entry": "frontend/index.html", "files": files },
    });
    sign_manifest(&mut manifest, signing_key);
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();

    let mut entries = vec![
        ("manifest.json", manifest_bytes.as_slice()),
        ("svc.wasm", wasm),
    ];
    entries.extend_from_slice(shipped);
    pack_entries(&dir, "frontend.mpk", &entries)
}

/// Install through the real entry point, returning the refusal if any.
async fn try_install(node_client: &NodeClient, bytes: Vec<u8>) -> eyre::Result<ApplicationId> {
    let blob_id = blob_id_in(node_client, &bytes).await;
    let source = "file:///test/bundle.mpk".parse().unwrap();
    node_client
        .install_application_from_bundle_blob(&blob_id, &source)
        .await
}

/// A refused install leaves only the caller's bundle blob: no application row
/// and no per-service blob.
fn assert_nothing_installed(node_client: &NodeClient) {
    assert!(node_client.list_applications().unwrap().is_empty());
    assert_eq!(node_client.list_blobs().unwrap().len(), 1);
}

#[tokio::test]
async fn a_bundle_with_a_valid_frontend_installs() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let files: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];

    let id = try_install(&node_client, frontend_bundle(files, files))
        .await
        .expect("a valid frontend installs");

    assert!(node_client.get_application(&id).unwrap().is_some());
}

#[tokio::test]
async fn a_resized_frontend_file_refuses_the_install() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let listed: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];
    let shipped: &[(&str, &[u8])] = &[("frontend/index.html", b"<html>evil</html>")];

    let err = try_install(&node_client, frontend_bundle(listed, shipped))
        .await
        .expect_err("tampered frontend bytes must not install");

    assert!(err.to_string().contains("FRONTEND_SIZE_MISMATCH"), "{err}");
    assert_nothing_installed(&node_client);
}

#[tokio::test]
async fn a_frontend_file_with_a_wrong_hash_refuses_the_install() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let listed: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];
    let same_size: &[(&str, &[u8])] =
        &[("frontend/index.html", b"<html><body>HELLO</body></html>")];
    assert_eq!(FRONTEND_INDEX.len(), same_size[0].1.len());

    let err = try_install(&node_client, frontend_bundle(listed, same_size))
        .await
        .expect_err("tampered frontend bytes must not install");

    assert!(err.to_string().contains("FRONTEND_HASH_MISMATCH"), "{err}");
    assert_nothing_installed(&node_client);
}

#[tokio::test]
async fn a_frontend_file_missing_from_the_archive_refuses_the_install() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let listed: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];

    let err = try_install(&node_client, frontend_bundle(listed, &[]))
        .await
        .expect_err("a listed file the archive lacks must not install");

    assert!(err.to_string().contains("FRONTEND_MISSING_FILE"), "{err}");
    assert!(err.to_string().contains("frontend/index.html"), "{err}");
    assert_nothing_installed(&node_client);
}

#[tokio::test]
async fn an_invalid_frontend_manifest_refuses_the_install_with_its_code() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let listed: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX), ("svc.wasm", b"x")];
    let shipped: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];

    let err = try_install(&node_client, frontend_bundle(listed, shipped))
        .await
        .expect_err("an unsafe frontend path must not install");

    assert!(err.to_string().contains("FRONTEND_UNSAFE_PATH"), "{err}");
    assert_nothing_installed(&node_client);
}

#[tokio::test]
async fn a_wasm_artifact_longer_than_its_declared_size_still_installs() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let files: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];

    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let id = try_install(
        &node_client,
        signed_frontend_bundle(&key, "1.0.0", files, files, Some(1)),
    )
    .await
    .expect("the wasm size field was never enforced, so understating it must not refuse");

    assert!(node_client.get_application(&id).unwrap().is_some());
}

/// Installs `frontend_bundle(files, files)` and returns the bundle blob id.
async fn install_frontend(
    node_client: &NodeClient,
    files: &[(&str, &[u8])],
) -> (ApplicationId, BlobId) {
    let bytes = frontend_bundle(files, files);
    let blob_id = blob_id_in(node_client, &bytes).await;
    let source = "file:///test/bundle.mpk".parse().unwrap();
    let id = node_client
        .install_application_from_bundle_blob(&blob_id, &source)
        .await
        .expect("frontend bundle installs");
    (id, blob_id)
}

async fn blob_id_in(node_client: &NodeClient, bytes: &[u8]) -> BlobId {
    node_client
        .add_blob(Cursor::new(bytes), Some(bytes.len() as u64), None)
        .await
        .expect("add the bundle blob")
        .0
}

async fn read_blob(node_client: &NodeClient, blob_id: BlobId) -> Vec<u8> {
    node_client
        .get_blob_bytes(&blob_id, None)
        .await
        .unwrap()
        .expect("blob present")
        .to_vec()
}

#[tokio::test]
async fn an_installed_frontend_is_stored_file_by_file() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let files = &FILES_1_0_0;

    let (id, bundle) = install_frontend(&node_client, files).await;

    let index = node_client.get_bundle_frontend(&bundle).unwrap().unwrap();
    assert_eq!(index.application.application_id(), id);
    assert_eq!(&*index.entry, "frontend/index.html");
    assert_eq!(index.files.len(), files.len());
    for (path, bytes) in files {
        let file = index.files.iter().find(|f| &*f.path == *path).unwrap();
        assert_eq!(read_blob(&node_client, file.blob.blob_id()).await, *bytes);
    }
}

#[tokio::test]
async fn a_bundle_without_a_frontend_writes_no_frontend_key() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let (bytes, _id) = common::minimal_signed_bundle_bytes("com.example.plain", "1.0.0");
    let bundle = blob_id_in(&node_client, &bytes).await;
    let source = "file:///test/bundle.mpk".parse().unwrap();

    node_client
        .install_application_from_bundle_blob(&bundle, &source)
        .await
        .unwrap();

    assert_eq!(node_client.get_bundle_frontend(&bundle).unwrap(), None);
    assert_eq!(node_client.list_blobs().unwrap().len(), 1);
}

#[tokio::test]
async fn reinstalling_a_bundle_takes_no_second_reference_on_its_frontend() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let files: &[(&str, &[u8])] = &[("frontend/index.html", FRONTEND_INDEX)];
    let (id, bundle) = install_frontend(&node_client, files).await;
    let source = "file:///test/bundle.mpk".parse().unwrap();
    node_client
        .install_application_from_bundle_blob(&bundle, &source)
        .await
        .unwrap();

    node_client.uninstall_application(&id).await.unwrap();

    assert_eq!(node_client.get_bundle_frontend(&bundle).unwrap(), None);
    // Only the bundle blob and the service wasm remain.
    assert_eq!(node_client.list_blobs().unwrap().len(), 2);
}

#[tokio::test]
async fn a_refused_remote_downgrade_stores_no_frontend() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let source = "file:///test/bundle.mpk".parse().unwrap();
    let newer = frontend_bundle_by(&key, "2.0.0", &FILES_2_0_0, &FILES_2_0_0);
    let newer_blob = blob_id_in(&node_client, &newer).await;
    let id = node_client
        .install_application_from_bundle_blob(&newer_blob, &source)
        .await
        .unwrap();
    let older = frontend_bundle_by(&key, "1.0.0", &FILES_1_0_0, &FILES_1_0_0);
    let older_blob = blob_id_in(&node_client, &older).await;
    let before = node_client.list_blobs().unwrap().len();

    node_client
        .install_expected_bundle_blob(&id, &older_blob, &source)
        .await
        .unwrap();

    assert_eq!(node_client.get_bundle_frontend(&older_blob).unwrap(), None);
    assert_eq!(node_client.list_blobs().unwrap().len(), before);
}

#[tokio::test]
async fn an_upgrade_keeps_every_version_s_frontend_and_shares_identical_files() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let (_, [v1, v2]) = install_two_releases(&node_client).await;
    let (i1, i2) = (
        node_client.get_bundle_frontend(&v1).unwrap().unwrap(),
        node_client.get_bundle_frontend(&v2).unwrap().unwrap(),
    );
    assert_eq!((&*i1.version, &*i2.version), ("1.0.0", "2.0.0"));
    let js = |i: &calimero_store::types::BundleFrontend| {
        i.files
            .iter()
            .find(|f| &*f.path == "frontend/app.js")
            .unwrap()
            .blob
            .blob_id()
    };
    assert_eq!(js(&i1), js(&i2));
    assert_eq!(read_blob(&node_client, js(&i1)).await, FRONTEND_APP_JS);
}

/// Installs v1 then v2 of one app (partly shared files); returns the app id and both bundle blobs.
async fn install_two_releases(node_client: &NodeClient) -> (ApplicationId, [BlobId; 2]) {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let source = "file:///test/bundle.mpk".parse().unwrap();
    let mut blobs = Vec::new();
    let mut id = None;
    for (version, files) in [("1.0.0", &FILES_1_0_0), ("2.0.0", &FILES_2_0_0)] {
        let bytes = frontend_bundle_by(&key, version, files, files);
        let blob = blob_id_in(node_client, &bytes).await;
        id = Some(
            node_client
                .install_application_from_bundle_blob(&blob, &source)
                .await
                .unwrap(),
        );
        blobs.push(blob);
    }
    (id.unwrap(), [blobs[0], blobs[1]])
}

fn frontend_blobs(node_client: &NodeClient, bundle: &BlobId) -> Vec<BlobId> {
    let index = node_client.get_bundle_frontend(bundle).unwrap().unwrap();
    index.files.iter().map(|f| f.blob.blob_id()).collect()
}

#[tokio::test]
async fn uninstall_releases_every_release_s_frontend() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let (id, bundles) = install_two_releases(&node_client).await;
    let blobs: Vec<_> = bundles
        .iter()
        .flat_map(|b| frontend_blobs(&node_client, b))
        .collect();

    node_client.uninstall_application(&id).await.unwrap();

    for bundle in &bundles {
        assert_eq!(node_client.get_bundle_frontend(bundle).unwrap(), None);
    }
    for blob in blobs {
        assert!(!node_client.has_blob(&blob).unwrap());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn installing_one_bundle_twice_at_once_leaks_no_frontend_blob() {
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;
    let bytes = frontend_bundle(&FILES_1_0_0, &FILES_1_0_0);
    let bundle = blob_id_in(&node_client, &bytes).await;
    let source: ApplicationSource = "file:///test/bundle.mpk".parse().unwrap();

    let install = || {
        let (node_client, source) = (node_client.clone(), source.clone());
        tokio::spawn(async move {
            node_client
                .install_application_from_bundle_blob(&bundle, &source)
                .await
        })
    };
    let (first, second) = (install(), install());
    let id = first.await.unwrap().unwrap();
    assert_eq!(second.await.unwrap().unwrap(), id);
    let blobs = frontend_blobs(&node_client, &bundle);

    node_client.uninstall_application(&id).await.unwrap();

    assert_eq!(node_client.get_bundle_frontend(&bundle).unwrap(), None);
    for blob in blobs {
        assert!(!node_client.has_blob(&blob).unwrap());
    }
}

#[derive(Debug, Clone, Copy)]
enum IndexFault {
    Put,
    Delete,
}

/// A datastore whose frontend index fails the given write: a delete fails the
/// cleanup of a refused install after the index was written, a put fails the write itself.
#[derive(Debug)]
struct FaultyFrontendIndex(Arc<dyn for<'x> Database<'x>>, IndexFault);

impl<'a> Database<'a> for FaultyFrontendIndex {
    fn open(_config: &StoreConfig) -> eyre::Result<Self> {
        eyre::bail!("test double: not openable")
    }

    fn has(&self, col: Column, key: Slice<'_>) -> eyre::Result<bool> {
        self.0.has(col, key)
    }

    fn get(&self, col: Column, key: Slice<'_>) -> eyre::Result<Option<Slice<'_>>> {
        self.0.get(col, key)
    }

    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> eyre::Result<()> {
        if matches!((col, self.1), (Column::BundleFrontend, IndexFault::Put)) {
            eyre::bail!("frontend index put failed");
        }
        self.0.put(col, key, value)
    }

    fn delete(&self, col: Column, key: Slice<'_>) -> eyre::Result<()> {
        if matches!((col, self.1), (Column::BundleFrontend, IndexFault::Delete)) {
            eyre::bail!("frontend index delete failed");
        }
        self.0.delete(col, key)
    }

    fn iter(&self, col: Column) -> eyre::Result<Iter<'_>> {
        self.0.iter(col)
    }

    fn apply(&self, tx: &Transaction<'a>) -> eyre::Result<()> {
        self.0.apply(tx)
    }
}

/// Installs 1.0.0, then a remote 1.5.0 that stores its frontend and waits on the
/// row lock while a 2.0.0 row lands, so the row write is refused. Returns the
/// 1.5.0 bundle blob and the blob count before that install.
async fn install_a_release_whose_row_write_is_refused(
    node_client: &NodeClient,
    store: &Store,
) -> (BlobId, usize) {
    let key = SigningKey::generate(&mut UnwrapErr(SysRng));
    let source = "file:///test/bundle.mpk".parse().unwrap();
    let v1 = frontend_bundle_by(&key, "1.0.0", &FILES_1_0_0, &FILES_1_0_0);
    let v1_blob = blob_id_in(node_client, &v1).await;
    let id = node_client
        .install_application_from_bundle_blob(&v1_blob, &source)
        .await
        .unwrap();
    let v15 = frontend_bundle_by(&key, "1.5.0", &FILES_1_5_0, &FILES_1_5_0);
    let v15_blob = blob_id_in(node_client, &v15).await;
    let before = node_client.list_blobs().unwrap().len();

    let rows = lock_application_rows();
    let install = tokio::spawn({
        let node_client = node_client.clone();
        async move {
            node_client
                .install_expected_bundle_blob(&id, &v15_blob, &source)
                .await
        }
    });
    for _ in 0..500 {
        if node_client.list_blobs().unwrap().len() > before {
            break;
        }
        std::thread::sleep(Duration::from_millis(10)); // the lock guard may not cross an await
    }
    let row_key = calimero_store::key::ApplicationMeta::new(id);
    let mut handle = store.handle();
    let mut row = handle.get(&row_key).unwrap().unwrap();
    row.version = "2.0.0".into();
    handle.put(&row_key, &row).unwrap();
    drop(rows);

    install.await.unwrap().unwrap();
    (v15_blob, before)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_write_refused_after_the_files_were_stored_releases_them() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(Some(store.clone())).await;

    let (v15_blob, before) =
        install_a_release_whose_row_write_is_refused(&node_client, &store).await;

    assert_eq!(node_client.get_bundle_frontend(&v15_blob).unwrap(), None);
    assert_eq!(node_client.list_blobs().unwrap().len(), before);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_index_that_cannot_be_deleted_keeps_its_files() {
    let store = Store::new(Arc::new(FaultyFrontendIndex(
        Arc::new(InMemoryDB::owned()),
        IndexFault::Delete,
    )));
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(Some(store.clone())).await;

    let (v15_blob, before) =
        install_a_release_whose_row_write_is_refused(&node_client, &store).await;

    // The index stays, so the files it names must stay with it. 1.5.0's
    // index.html is the one file no earlier release shares.
    assert!(node_client.list_blobs().unwrap().len() > before);
    for blob in frontend_blobs(&node_client, &v15_blob) {
        assert!(node_client.has_blob(&blob).unwrap());
    }
}

#[tokio::test]
async fn an_index_write_that_fails_releases_the_service_blobs_too() {
    let store = Store::new(Arc::new(FaultyFrontendIndex(
        Arc::new(InMemoryDB::owned()),
        IndexFault::Put,
    )));
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(Some(store)).await;
    let bytes = frontend_bundle(&FILES_1_0_0, &FILES_1_0_0);
    let bundle = blob_id_in(&node_client, &bytes).await;
    let before = node_client.list_blobs().unwrap().len();

    let result = node_client
        .install_application_from_bundle_blob(&bundle, &"file:///test/bundle.mpk".parse().unwrap())
        .await;

    assert!(result.is_err());
    assert_eq!(node_client.list_blobs().unwrap().len(), before);
}
