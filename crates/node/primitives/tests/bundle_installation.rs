//! Tests for bundle installation and extraction

use std::fs;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::NodeClient;
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use flate2::write::GzEncoder;
use flate2::Compression;
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
async fn create_test_node_client() -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();

    let datastore = Store::open::<RocksDB>(&StoreConfig::new(
        data_dir.path().to_path_buf().try_into().unwrap(),
    ))
    .unwrap();

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
    );

    (node_client, data_dir, blob_dir)
}

#[tokio::test]
async fn test_bundle_detection() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client().await;

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
    let (node_client, _data_dir, blob_dir) = create_test_node_client().await;

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
    let (node_client, _data_dir, _blob_dir) = create_test_node_client().await;

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
    let (node_client, _data_dir, blob_dir) = create_test_node_client().await;

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
    let (node_client, _data_dir, _blob_dir) = create_test_node_client().await;

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
async fn test_bundle_backward_compatibility() {
    let temp_dir = TempDir::new().unwrap();
    let (node_client, _data_dir, _blob_dir) = create_test_node_client().await;

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
