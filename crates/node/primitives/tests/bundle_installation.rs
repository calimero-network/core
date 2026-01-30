//! Tests for bundle installation and extraction

use std::fs;

use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::io::Cursor;
use serde_json;
use tar::Builder;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

use calimero_node_primitives::bundle::BundleManifest;

/// Create a test bundle archive with manifest.json, app.wasm, abi.json, and migrations
fn create_test_bundle(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_content: &[u8],
    abi_content: Option<&[u8]>,
    migrations: Vec<(&str, &[u8])>,
) -> Utf8PathBuf {
    let bundle_path = temp_dir.path().join(format!("{}-{}.mpk", package, version));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // Create manifest.json
    let manifest = BundleManifest {
        version: "1.0".to_string(),
        package: package.to_string(),
        app_version: version.to_string(),
        signer_id: None,
        min_runtime_version: "1.0.0".to_string(),
        metadata: None,
        interfaces: None,
        wasm: Some(calimero_node_primitives::bundle::BundleArtifact {
            path: "app.wasm".to_string(),
            hash: None,
            size: wasm_content.len() as u64,
        }),
        abi: abi_content.map(|content| calimero_node_primitives::bundle::BundleArtifact {
            path: "abi.json".to_string(),
            hash: None,
            size: content.len() as u64,
        }),
        migrations: migrations
            .iter()
            .map(
                |(path, content)| calimero_node_primitives::bundle::BundleArtifact {
                    path: path.to_string(),
                    hash: None,
                    size: content.len() as u64,
                },
            )
            .collect(),
        links: None,
        signature: None,
    };

    let manifest_json = serde_json::to_vec(&manifest).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_json.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_json.as_slice())
        .unwrap();

    // Add WASM file
    let mut wasm_header = tar::Header::new_gnu();
    wasm_header.set_path("app.wasm").unwrap();
    wasm_header.set_size(wasm_content.len() as u64);
    wasm_header.set_cksum();
    tar.append(&wasm_header, wasm_content).unwrap();

    // Add ABI file if provided
    if let Some(abi_content) = abi_content {
        let mut abi_header = tar::Header::new_gnu();
        abi_header.set_path("abi.json").unwrap();
        abi_header.set_size(abi_content.len() as u64);
        abi_header.set_cksum();
        tar.append(&abi_header, abi_content).unwrap();
    }

    // Add migrations
    for (path, content) in migrations {
        let mut migration_header = tar::Header::new_gnu();
        migration_header.set_path(path).unwrap();
        migration_header.set_size(content.len() as u64);
        migration_header.set_cksum();
        tar.append(&migration_header, content).unwrap();
    }

    tar.finish().unwrap();
    bundle_path.try_into().unwrap()
}

/// Create a test NodeClient with temporary directories
///
/// The `datastore` parameter allows injecting a custom Store implementation.
/// If `None` is provided, defaults to `InMemoryDB` (no file I/O, faster tests).
async fn create_test_node_client(datastore: Option<Store>) -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();

    // Default to InMemoryDB if no store is provided (avoids dependency on calimero-store-rocksdb)
    let datastore = datastore.unwrap_or_else(|| Store::new(Arc::new(InMemoryDB::owned())));

    let blobstore = BlobManager::new(
        datastore.clone(),
        FileSystem::new(&BlobStoreConfig::new(
            blob_dir.path().to_path_buf().try_into().unwrap(),
        ))
        .await
        .unwrap(),
    );

    let (event_sender, _) = broadcast::channel(256);
    let (ctx_sync_tx, _) = mpsc::channel(64);

    let node_client = NodeClient::new(
        datastore,
        blobstore,
        NetworkClient::new(LazyRecipient::new()),
        LazyRecipient::new(),
        event_sender,
        ctx_sync_tx,
        String::new(), // Not used in tests
    );

    (node_client, data_dir, blob_dir)
}

#[tokio::test]
async fn test_bundle_detection() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Test single WASM file (should not be detected as bundle)
    let wasm_path = temp_dir.path().join("app.wasm");
    fs::write(&wasm_path, b"wasm content").unwrap();
    let wasm_path_utf8: Utf8PathBuf = wasm_path.try_into().unwrap();

    let result = node_client
        .install_application_from_path(wasm_path_utf8, vec![])
        .await;
    assert!(result.is_ok(), "Single WASM installation should work");

    // Test bundle file (should be detected as bundle)
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.bundle",
        "1.0.0",
        b"wasm content",
        None,
        vec![],
    );

    let result = node_client
        .install_application_from_path(bundle_path, vec![])
        .await;
    assert!(result.is_ok(), "Bundle installation should work");
}

#[tokio::test]
async fn test_bundle_installation() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

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
        migrations.clone(),
    );

    // Install the bundle
    let application_id = node_client
        .install_application_from_path(bundle_path.clone(), vec![])
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

    // Verify extracted files exist
    // Applications are now extracted to node root (parent of blobstore), not blobstore root
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir = node_root
        .join("applications")
        .join("com.example.test")
        .join("1.0.0")
        .join("extracted");

    let wasm_path = extract_dir.join("app.wasm");
    assert!(wasm_path.exists(), "Extracted WASM should exist");

    let abi_path = extract_dir.join("abi.json");
    assert!(abi_path.exists(), "Extracted ABI should exist");

    let migration1_path = extract_dir.join("migrations/001_init.sql");
    assert!(migration1_path.exists(), "First migration should exist");

    let migration2_path = extract_dir.join("migrations/002_add_users.sql");
    assert!(migration2_path.exists(), "Second migration should exist");

    // Verify file contents
    let extracted_wasm = fs::read(&wasm_path).unwrap();
    assert_eq!(extracted_wasm, wasm_content, "WASM content should match");

    let extracted_abi = fs::read(&abi_path).unwrap();
    assert_eq!(extracted_abi, abi_content, "ABI content should match");

    let extracted_migration1 = fs::read(&migration1_path).unwrap();
    assert_eq!(
        extracted_migration1, migrations[0].1,
        "Migration 1 content should match"
    );
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
        .install_application_from_path(bundle_path, vec![])
        .await
        .expect("Bundle installation should succeed");

    // Get application bytes (should read from extracted directory)
    let bytes = node_client
        .get_application_bytes(&application_id)
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
async fn test_bundle_deduplication() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

    // Create first bundle with specific WASM content
    let wasm_content_v1 = b"shared wasm bytecode";
    let bundle_path_v1 = create_test_bundle(
        &temp_dir,
        "com.example.shared",
        "1.0.0",
        wasm_content_v1,
        None,
        vec![],
    );

    // Install first version
    let _app_id_v1 = node_client
        .install_application_from_path(bundle_path_v1, vec![])
        .await
        .expect("First bundle installation should succeed");

    // Create second bundle with same WASM content (should be deduplicated)
    let wasm_content_v2 = wasm_content_v1; // Same content
    let bundle_path_v2 = create_test_bundle(
        &temp_dir,
        "com.example.shared",
        "2.0.0",
        wasm_content_v2,
        None,
        vec![],
    );

    // Install second version
    let _app_id_v2 = node_client
        .install_application_from_path(bundle_path_v2, vec![])
        .await
        .expect("Second bundle installation should succeed");

    // Check that both versions have the WASM file
    // Applications are now extracted to node root (parent of blobstore), not blobstore root
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir_v1 = node_root
        .join("applications")
        .join("com.example.shared")
        .join("1.0.0")
        .join("extracted");

    let extract_dir_v2 = node_root
        .join("applications")
        .join("com.example.shared")
        .join("2.0.0")
        .join("extracted");

    let wasm_path_v1 = extract_dir_v1.join("app.wasm");
    let wasm_path_v2 = extract_dir_v2.join("app.wasm");

    assert!(wasm_path_v1.exists(), "V1 WASM should exist");
    assert!(wasm_path_v2.exists(), "V2 WASM should exist");

    // Verify both files exist and have the same content
    // (Deduplication may use hardlinks or copies depending on filesystem)
    let content_v1 = fs::read(&wasm_path_v1).unwrap();
    let content_v2 = fs::read(&wasm_path_v2).unwrap();
    assert_eq!(
        content_v1, content_v2,
        "Both versions should have same WASM content"
    );
    assert_eq!(content_v1, wasm_content_v1, "Content should match original");
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
    let result = node_client
        .install_application_from_path(bundle_path, vec![])
        .await;

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

    // Create manifest.json with missing package field
    // Use a raw JSON string to ensure package field is truly missing
    let invalid_manifest_json = r#"{
        "version": "1.0",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "1.0.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10
        },
        "migrations": []
    }"#;
    let manifest_json = invalid_manifest_json.as_bytes();
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
        .install_application_from_path(bundle_path_utf8, vec![])
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for invalid manifest"
    );
    let error_msg = result.unwrap_err().to_string();
    // BundleManifest deserialization will fail if package field is missing
    // The error could be "missing field 'package'" from serde or "package field is empty" from validation
    // Or it could be a tar parsing error if the archive is malformed
    assert!(
        error_msg.contains("package")
            || error_msg.contains("missing field")
            || error_msg.contains("empty")
            || error_msg.contains("manifest")
            || error_msg.contains("parse"),
        "Error should mention missing package field, manifest issue, or parse error, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_bundle_backward_compatibility() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Test that single WASM files still work
    let wasm_path = temp_dir.path().join("app.wasm");
    fs::write(&wasm_path, b"single wasm content").unwrap();
    let wasm_path_utf8: Utf8PathBuf = wasm_path.try_into().unwrap();

    let application_id = node_client
        .install_application_from_path(wasm_path_utf8, vec![])
        .await
        .expect("Single WASM installation should work");

    // Verify it was installed correctly
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist");
    assert!(
        application.is_some(),
        "Single WASM application should be found"
    );

    // Verify we can get bytes
    let bytes = node_client
        .get_application_bytes(&application_id)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(bytes.as_ref(), b"single wasm content", "Bytes should match");
}

/// Create a test bundle with custom WASM path
fn create_test_bundle_custom_wasm_path(
    temp_dir: &TempDir,
    package: &str,
    version: &str,
    wasm_path: &str,
    wasm_content: &[u8],
) -> Utf8PathBuf {
    let bundle_path = temp_dir.path().join(format!("{}-{}.mpk", package, version));
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    // Create manifest.json with custom WASM path
    let manifest = BundleManifest {
        version: "1.0".to_string(),
        package: package.to_string(),
        app_version: version.to_string(),
        signer_id: None,
        min_runtime_version: "1.0.0".to_string(),
        metadata: None,
        interfaces: None,
        wasm: Some(calimero_node_primitives::bundle::BundleArtifact {
            path: wasm_path.to_string(),
            hash: None,
            size: wasm_content.len() as u64,
        }),
        abi: None,
        migrations: vec![],
        links: None,
        signature: None,
    };

    let manifest_json = serde_json::to_vec(&manifest).unwrap();
    let mut manifest_header = tar::Header::new_gnu();
    manifest_header.set_path("manifest.json").unwrap();
    manifest_header.set_size(manifest_json.len() as u64);
    manifest_header.set_cksum();
    tar.append(&manifest_header, manifest_json.as_slice())
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
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

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
        .install_application_from_path(bundle_path, vec![])
        .await
        .expect("Bundle installation should succeed");

    // Verify WASM was extracted at custom path
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir = node_root
        .join("applications")
        .join("com.example.custom")
        .join("1.0.0")
        .join("extracted");
    let wasm_path = extract_dir.join("src/main.wasm");
    assert!(
        wasm_path.exists(),
        "WASM should be extracted at custom path"
    );

    // Verify get_application_bytes reads from custom path
    let bytes = node_client
        .get_application_bytes(&application_id)
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
        .install_application_from_path(bundle_path, vec![])
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

#[tokio::test]
async fn test_bundle_validation_empty_package() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Create bundle with empty package field
    let bundle_path = temp_dir.path().join("empty-package.mpk");
    let bundle_file = fs::File::create(&bundle_path).unwrap();
    let encoder = GzEncoder::new(bundle_file, Compression::default());
    let mut tar = Builder::new(encoder);

    let invalid_manifest_json = r#"{
        "version": "1.0",
        "package": "",
        "appVersion": "1.0.0",
        "minRuntimeVersion": "1.0.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10
        },
        "migrations": []
    }"#;
    let manifest_json = invalid_manifest_json.as_bytes();
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
        .install_application_from_path(bundle_path_utf8, vec![])
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for empty package"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("package") && error_msg.contains("empty"),
        "Error should mention empty package field, got: {}",
        error_msg
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

    let invalid_manifest_json = r#"{
        "version": "1.0",
        "package": "com.example.test",
        "appVersion": "",
        "minRuntimeVersion": "1.0.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10
        },
        "migrations": []
    }"#;
    let manifest_json = invalid_manifest_json.as_bytes();
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
        .install_application_from_path(bundle_path_utf8, vec![])
        .await;

    assert!(
        result.is_err(),
        "Bundle installation should fail for empty appVersion"
    );
    let error_msg = result.unwrap_err().to_string();
    assert!(
        error_msg.contains("appVersion") || error_msg.contains("version"),
        "Error should mention empty appVersion field, got: {}",
        error_msg
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

    let invalid_manifest_json = r#"{
        "version": "1.0",
        "package": "com.example.test",
        "minRuntimeVersion": "1.0.0",
        "wasm": {
            "path": "app.wasm",
            "size": 10
        },
        "migrations": []
    }"#;
    let manifest_json = invalid_manifest_json.as_bytes();
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
        .install_application_from_path(bundle_path_utf8, vec![])
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
        "Error should mention missing appVersion field, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_bundle_deduplication_different_paths() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

    // Create first bundle with WASM at one path
    let wasm_content = b"shared wasm";
    let bundle_path_v1 = create_test_bundle_custom_wasm_path(
        &temp_dir,
        "com.example.paths",
        "1.0.0",
        "src/app.wasm",
        wasm_content,
    );

    let _app_id_v1 = node_client
        .install_application_from_path(bundle_path_v1, vec![])
        .await
        .expect("First bundle installation should succeed");

    // Create second bundle with WASM at different path but same content
    let bundle_path_v2 = create_test_bundle_custom_wasm_path(
        &temp_dir,
        "com.example.paths",
        "2.0.0",
        "lib/app.wasm", // Different path
        wasm_content,   // Same content
    );

    let _app_id_v2 = node_client
        .install_application_from_path(bundle_path_v2, vec![])
        .await
        .expect("Second bundle installation should succeed");

    // Verify both paths exist (should NOT be deduplicated because paths differ)
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir_v1 = node_root
        .join("applications")
        .join("com.example.paths")
        .join("1.0.0")
        .join("extracted");
    let extract_dir_v2 = node_root
        .join("applications")
        .join("com.example.paths")
        .join("2.0.0")
        .join("extracted");

    let wasm_path_v1 = extract_dir_v1.join("src/app.wasm");
    let wasm_path_v2 = extract_dir_v2.join("lib/app.wasm");

    assert!(
        wasm_path_v1.exists(),
        "V1 WASM should exist at src/app.wasm"
    );
    assert!(
        wasm_path_v2.exists(),
        "V2 WASM should exist at lib/app.wasm"
    );

    // Both should have same content but be separate files (not deduplicated due to different paths)
    let content_v1 = fs::read(&wasm_path_v1).unwrap();
    let content_v2 = fs::read(&wasm_path_v2).unwrap();
    assert_eq!(content_v1, content_v2, "Both should have same content");
    assert_eq!(content_v1, wasm_content, "Content should match original");
}

#[tokio::test]
async fn test_bundle_extract_dir_derived_from_manifest() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

    // Install bundle
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.derived",
        "2.5.0",
        b"wasm content",
        None,
        vec![],
    );

    let application_id = node_client
        .install_application_from_path(bundle_path, vec![])
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
        metadata_json["package"], "com.example.derived",
        "Package should match manifest"
    );
    assert_eq!(
        metadata_json["version"], "2.5.0",
        "Version should match manifest"
    );

    // Verify files were extracted to correct location derived from manifest
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir = node_root
        .join("applications")
        .join("com.example.derived")
        .join("2.5.0")
        .join("extracted");
    assert!(
        extract_dir.exists(),
        "Extract dir should exist at derived path"
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
        .install_application_from_path(bundle_path, vec![])
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
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

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

    // Verify bundle was extracted
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir = node_root
        .join("applications")
        .join("com.example.blob")
        .join("1.0.0")
        .join("extracted");

    let wasm_path = extract_dir.join("app.wasm");
    assert!(wasm_path.exists(), "Extracted WASM should exist");

    let abi_path = extract_dir.join("abi.json");
    assert!(abi_path.exists(), "Extracted ABI should exist");

    // Verify file contents
    let extracted_wasm = fs::read(&wasm_path).unwrap();
    assert_eq!(extracted_wasm, wasm_content, "WASM content should match");

    // Verify we can get application bytes
    let bytes = node_client
        .get_application_bytes(&application_id)
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
        "Error should mention blob not found, got: {}",
        error_msg
    );
}

#[tokio::test]
async fn test_simple_wasm_installation_still_works() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client(None).await;

    // Test that simple WASM installation still works (backward compatibility)
    let wasm_path = temp_dir.path().join("app.wasm");
    fs::write(&wasm_path, b"simple wasm bytecode").unwrap();
    let wasm_path_utf8: Utf8PathBuf = wasm_path.try_into().unwrap();

    let application_id = node_client
        .install_application_from_path(wasm_path_utf8, vec![])
        .await
        .expect("Single WASM installation should work");

    // Verify it was installed correctly
    let application = node_client
        .get_application(&application_id)
        .expect("Application should exist");
    assert!(
        application.is_some(),
        "Single WASM application should be found"
    );

    // Verify we can get bytes
    let bytes = node_client
        .get_application_bytes(&application_id)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes.as_ref(),
        b"simple wasm bytecode",
        "Bytes should match"
    );

    // Verify it's not detected as a bundle
    let app = application.unwrap();
    let blob_bytes = node_client
        .get_blob_bytes(&app.blob.bytecode, None)
        .await
        .expect("Should get blob bytes")
        .expect("Blob bytes should exist");

    assert!(
        !NodeClient::is_bundle_blob(&blob_bytes),
        "Simple WASM should not be detected as bundle"
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
/// 7. sync_context_config calls install_application_from_bundle_blob
/// 8. User 2 can now use the context
///
/// To test the full flow, we would need to:
/// - Set up ContextClient instances (requires external client mocking)
/// - Mock external config client methods (application(), application_revision(), members_revision(), members())
/// - Simulate context creation and invitation
///
/// The current test verifies the critical integration point: when a bundle blob
/// exists locally, install_application_from_bundle_blob correctly installs it
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
        .install_application_from_path(bundle_path.clone(), vec![])
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
        !node_client_2
            .has_application(&application_id_user1)
            .unwrap(),
        "User 2 should not have application before sync"
    );

    // Step 4: Simulate sync_context_config by manually installing from blob
    // This simulates what happens when sync_context_config detects the blob
    // In real scenario, sync_context_config would call install_application_from_bundle_blob
    // No metadata needed - bundle detection happens via is_bundle_blob()
    let application_id_user2 = node_client_2
        .install_application_from_bundle_blob(
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
        .get_application_bytes(&application_id_user2)
        .await
        .expect("Should get application bytes")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes_user2.as_ref(),
        wasm_content,
        "User 2 should be able to read WASM from bundle"
    );

    // Step 7: Verify bundle was extracted on User 2's node
    let node_root_2 = blob_dir_2.path().parent().unwrap();
    let extract_dir_2 = node_root_2
        .join("applications")
        .join("com.example.integration")
        .join("1.0.0")
        .join("extracted");

    let wasm_path_2 = extract_dir_2.join("app.wasm");
    assert!(
        wasm_path_2.exists(),
        "User 2 should have extracted WASM file"
    );

    let extracted_wasm_2 = fs::read(&wasm_path_2).unwrap();
    assert_eq!(
        extracted_wasm_2, wasm_content,
        "Extracted WASM content should match"
    );
}

// Note: install_application_from_url tests require HTTP/HTTPS URLs and would need
// a mock HTTP server or real server. The URL installation logic is covered by
// integration tests. Path-based installation (which covers the same code paths
// for bundle detection and extraction) is tested below.

#[tokio::test]
async fn test_bundle_get_application_bytes_fallback() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, blob_dir) = create_test_node_client(None).await;

    // Create a test bundle
    let wasm_content = b"fallback test wasm bytecode";
    let bundle_path = create_test_bundle(
        &temp_dir,
        "com.example.fallback",
        "1.0.0",
        wasm_content,
        None,
        vec![],
    );

    // Install the bundle
    let application_id = node_client
        .install_application_from_path(bundle_path, vec![])
        .await
        .expect("Bundle installation should succeed");

    // Verify extracted WASM exists initially
    let node_root = blob_dir.path().parent().unwrap();
    let extract_dir = node_root
        .join("applications")
        .join("com.example.fallback")
        .join("1.0.0")
        .join("extracted");
    let wasm_path = extract_dir.join("app.wasm");
    assert!(wasm_path.exists(), "Extracted WASM should exist initially");

    // Delete extracted WASM to trigger fallback
    fs::remove_file(&wasm_path).expect("Should delete extracted WASM");
    assert!(!wasm_path.exists(), "WASM should be deleted");

    // Get application bytes - should fallback to re-extract from bundle blob
    // Note: fallback now extracts entire bundle to disk for persistence
    let bytes = node_client
        .get_application_bytes(&application_id)
        .await
        .expect("Should get application bytes via fallback")
        .expect("Application bytes should exist");

    assert_eq!(
        bytes.as_ref(),
        wasm_content,
        "Application bytes should match WASM content (re-extracted from bundle blob)"
    );

    // Verify WASM file now exists (fallback extracts bundle to disk for persistence)
    assert!(
        wasm_path.exists(),
        "WASM file should exist after fallback (fallback extracts bundle to disk)"
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
            .install_application_from_path(bundle_path, vec![])
            .await
            .expect("Bundle installation should succeed");

        application_ids.push((version.to_string(), app_id));
    }

    // Get latest version - should be "10.0.0" (not "2.5.0" which would be lexicographically latest)
    let latest_app_id = node_client
        .get_latest_version(package)
        .expect("Should get latest version")
        .expect("Latest version should exist");

    // Find which version this corresponds to
    let latest_version_str = application_ids
        .iter()
        .find(|(_, app_id)| *app_id == latest_app_id)
        .map(|(v, _)| v)
        .expect("Should find version for latest app_id");

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
            .install_application_from_path(bundle_path, vec![])
            .await
            .expect("Bundle installation should succeed");

        application_ids.push((version.to_string(), app_id));
    }

    // Get latest version - should prefer semantic versions over non-semantic
    let latest_app_id = node_client
        .get_latest_version(package)
        .expect("Should get latest version")
        .expect("Latest version should exist");

    let latest_version_str = application_ids
        .iter()
        .find(|(_, app_id)| *app_id == latest_app_id)
        .map(|(v, _)| v)
        .expect("Should find version for latest app_id");

    // Should be "2.0.0" (latest semantic version), not "invalid-version" or "also-invalid"
    assert_eq!(
        latest_version_str, "2.0.0",
        "Latest version should be 2.0.0 (semantic), preferring semantic versions over non-semantic"
    );
}
