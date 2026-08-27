//! Reproduction for issue #3311: the manifest signature does not bind artifact
//! bytes. A bundle whose `app.wasm` is swapped after signing still verifies and
//! still installs, because nothing compares the installed bytes against
//! `manifest.wasm.hash`.

use std::fs;
use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::bundle::{derive_signer_id_did_key, sign_manifest_json};
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use ed25519_dalek::SigningKey;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::io::Cursor;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};
use tar::Builder;
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Sign a manifest exactly the way `cargo mero bundle` does: real lowercase-hex
/// SHA-256 per artifact, then an Ed25519 signature over the canonical manifest.
fn signed_manifest(package: &str, version: &str, wasm: &[u8], key: &SigningKey) -> Vec<u8> {
    let signer_id = derive_signer_id_did_key(key.verifying_key().as_bytes());
    let mut manifest = serde_json::json!({
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": signer_id,
        "minRuntimeVersion": "0.1.0",
        "wasm": {
            "path": "app.wasm",
            "size": wasm.len(),
            "hash": hex_lower(&Sha256::digest(wasm)),
        },
        "migrations": []
    });
    sign_manifest_json(&mut manifest, key).unwrap();
    serde_json::to_vec(&manifest).unwrap()
}

/// Pack a `.mpk` from an exact manifest byte string plus wasm bytes. Keeping the
/// manifest as raw bytes is the point: the attacker's copy is byte-identical.
fn pack(dir: &TempDir, name: &str, manifest_bytes: &[u8], wasm: &[u8]) -> Vec<u8> {
    let path = dir.path().join(name);
    let encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::default());
    let mut tar = Builder::new(encoder);

    for (entry_path, content) in [("manifest.json", manifest_bytes), ("app.wasm", wasm)] {
        let mut header = tar::Header::new_gnu();
        header.set_path(entry_path).unwrap();
        header.set_size(content.len() as u64);
        header.set_cksum();
        tar.append(&header, content).unwrap();
    }
    tar.finish().unwrap();
    drop(tar);
    fs::read(&path).unwrap()
}

async fn node_client() -> (NodeClient, TempDir, TempDir) {
    let data_dir = TempDir::new().unwrap();
    let blob_dir = TempDir::new().unwrap();
    let datastore = Store::new(Arc::new(InMemoryDB::owned()));

    // Nest the blobstore one level down: the node derives its root as the blob
    // root's parent, so a bare TempDir would make every node share the OS temp
    // dir as its root (and thus its `applications/` extraction cache).
    let blob_root = blob_dir.path().join("blobs");
    let blob_store = BlobStore::new(
        datastore.clone(),
        FileSystem::new(&BlobStoreConfig::new(blob_root.try_into().unwrap()))
            .await
            .unwrap(),
    );
    let blob_manager = BlobManager::new(blob_store);

    let (event_sender, _) = broadcast::channel(256);
    let (ctx_sync_tx, _) = mpsc::channel(64);
    let (ns_sync_tx, _) = mpsc::channel(64);
    let (ns_join_tx, _) = mpsc::channel(16);
    let (open_subgroup_join_tx, _) = mpsc::channel(16);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        datastore,
        blob_manager,
        NetworkClient::new(LazyRecipient::new()),
        LazyRecipient::new(),
        event_sender,
        sync_client,
        None,
    );
    (node_client, data_dir, blob_dir)
}

/// Install through the production path (mandatory signature) and return the
/// ApplicationId plus the bytes the node would hand to the WASM runtime.
async fn install(
    client: &NodeClient,
    bundle: Vec<u8>,
) -> eyre::Result<(calimero_primitives::application::ApplicationId, Vec<u8>)> {
    let (blob_id, _) = client
        .add_blob(
            Cursor::new(bundle.as_slice()),
            Some(bundle.len() as u64),
            None,
        )
        .await?;
    let source = "https://registry.example.com/app.mpk".parse().unwrap();
    let app_id = client
        .install_application_from_bundle_blob(&blob_id, &source)
        .await?;
    let bytes = client
        .get_application_bytes(&app_id, None)
        .await?
        .expect("application bytes");
    Ok((app_id, bytes.to_vec()))
}

#[tokio::test]
async fn tampered_wasm_is_rejected_even_though_manifest_signature_verifies() {
    let dir = TempDir::new().unwrap();
    let key = SigningKey::generate(&mut OsRng);

    let honest_wasm = b"HONEST wasm bytecode from the publisher".as_slice();
    let evil_wasm = b"EVIL!! wasm bytecode from the registry ".as_slice();
    assert_eq!(
        honest_wasm.len(),
        evil_wasm.len(),
        "same length, so even manifest `size` still matches"
    );

    // One manifest, signed once. Both bundles carry these exact bytes.
    let manifest_bytes = signed_manifest("com.example.tamper", "1.0.0", honest_wasm, &key);
    let honest = pack(&dir, "honest.mpk", &manifest_bytes, honest_wasm);
    let evil = pack(&dir, "evil.mpk", &manifest_bytes, evil_wasm);

    // Two fresh nodes: one fetches from an honest mirror, one from a mirror that
    // swapped the wasm. Separate nodes because the extraction cache is keyed on
    // (package, version) and would otherwise hide the second install's bytes.
    let (honest_node, _d1, _b1) = node_client().await;
    let (victim_node, _d2, _b2) = node_client().await;

    let (honest_id, honest_bytes) = install(&honest_node, honest).await.expect("honest install");
    assert_eq!(honest_bytes, honest_wasm);

    match install(&victim_node, evil).await {
        Err(err) => {
            let msg = err.to_string();
            assert!(
                msg.contains("app.wasm"),
                "rejection must name the artifact, got: {msg}"
            );
        }
        Ok((evil_id, bytes)) => panic!(
            "REPRODUCED #3311: a signed manifest admitted substituted bytecode.\n  \
             manifest.wasm.hash  = {}\n  \
             sha256(served wasm) = {}\n  \
             bytes served to the runtime = {:?}\n  \
             ApplicationId honest = {honest_id}\n  \
             ApplicationId evil   = {evil_id}  (same = {})",
            hex_lower(&Sha256::digest(honest_wasm)),
            hex_lower(&Sha256::digest(&bytes)),
            String::from_utf8_lossy(&bytes),
            honest_id == evil_id,
        ),
    }
}

/// Rebuild a `.mpk` from a real one, replacing `app.wasm` and copying every
/// other entry (crucially `manifest.json`) byte-for-byte.
fn swap_wasm(original: &[u8], new_wasm: &[u8]) -> (Vec<u8>, String) {
    use std::io::Read;

    let mut manifest_hash = String::new();
    let mut out = Vec::new();
    {
        let encoder = GzEncoder::new(&mut out, Compression::default());
        let mut tar = Builder::new(encoder);
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(original));

        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().to_string();
            let mut content = Vec::new();
            entry.read_to_end(&mut content).unwrap();

            if path == "manifest.json" {
                let json: serde_json::Value = serde_json::from_slice(&content).unwrap();
                manifest_hash = json["wasm"]["hash"].as_str().unwrap().to_owned();
            }
            if path == "app.wasm" {
                content = new_wasm.to_vec();
            }

            let mut header = tar::Header::new_gnu();
            header.set_path(&path).unwrap();
            header.set_size(content.len() as u64);
            header.set_cksum();
            tar.append(&header, content.as_slice()).unwrap();
        }
        tar.finish().unwrap();
    }
    (out, manifest_hash)
}

/// Same attack against a genuine `cargo mero bundle` artifact: a real Ed25519
/// signature over a manifest carrying real artifact hashes.
#[tokio::test]
async fn real_cargo_mero_bundle_with_swapped_wasm_is_rejected() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tools/cargo-mero/tests/fixtures/demo-app/dist/com.example.demo-app.mpk"
    );
    let original = fs::read(fixture).expect("cargo-mero fixture bundle");

    // A wasm module that is valid enough to load but is not the publisher's.
    let evil_wasm = b"\0asm\x01\0\0\0EVIL".to_vec();
    let (tampered, manifest_hash) = swap_wasm(&original, &evil_wasm);

    let (node, _d, _b) = node_client().await;
    match install(&node, tampered).await {
        Err(err) => assert!(
            err.to_string().contains("app.wasm"),
            "rejection must name the artifact, got: {err}"
        ),
        Ok((app_id, bytes)) => panic!(
            "REPRODUCED #3311 against a real `cargo mero bundle` artifact.\n  \
             signature verified  = yes (production path requires it)\n  \
             manifest.wasm.hash  = {manifest_hash}\n  \
             sha256(served wasm) = {}\n  \
             served wasm len     = {} (publisher shipped 477593)\n  \
             ApplicationId       = {app_id}",
            hex_lower(&Sha256::digest(&bytes)),
            bytes.len(),
        ),
    }
}
