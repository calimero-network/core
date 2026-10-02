//! The development key is public, so its signature proves nothing about who built
//! a bundle: a node installs or runs one only when started with `--dev`.

use calimero_node_primitives::bundle::{dev_signer_id, dev_signing_key};
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::test_fixtures::{bundle, bundle_signed_by, node_client};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use futures_util::io::Cursor;
use tempfile::TempDir;

const PACKAGE: &str = "com.example.dev";
const WASM: &[u8] = b"wasm signed with the public development key";
const REFUSAL: &str = "run merod with --dev to install it";

fn dev_bundle(dir: &TempDir) -> Vec<u8> {
    let path = bundle_signed_by(dir, PACKAGE, "1.0.0", WASM, &dev_signing_key());
    std::fs::read(path).unwrap()
}

async fn store(node: &NodeClient, bytes: &[u8]) -> BlobId {
    let (blob_id, _) = node
        .add_blob(Cursor::new(bytes), Some(bytes.len() as u64), None)
        .await
        .unwrap();
    blob_id
}

fn assert_refused<T: std::fmt::Debug>(result: eyre::Result<T>) {
    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains(REFUSAL), "unexpected error: {err}");
}

#[tokio::test]
async fn an_operator_install_refuses_a_dev_signed_bundle() {
    let dir = TempDir::new().unwrap();
    let path = bundle_signed_by(&dir, PACKAGE, "1.0.0", WASM, &dev_signing_key());
    let (node, _store, _d, _b) = node_client().await;

    assert_refused(node.install_application_from_path(path).await);
}

#[tokio::test]
async fn a_bundle_signed_with_its_own_key_installs_without_dev() {
    let dir = TempDir::new().unwrap();
    let path = bundle(&dir, PACKAGE, "1.0.0", WASM);
    let (node, _store, _d, _b) = node_client().await;

    let app_id = node.install_application_from_path(path).await.unwrap();
    let bytes = node.get_application_bytes(&app_id, None).await.unwrap();
    assert_eq!(bytes.as_deref(), Some(WASM));
}

#[tokio::test]
async fn a_peer_named_dev_signed_blob_is_refused() {
    let dir = TempDir::new().unwrap();
    let (node, _store, _d, _b) = node_client().await;
    let blob_id = store(&node, &dev_bundle(&dir)).await;
    let expected = ApplicationId::for_bundle(PACKAGE, &dev_signer_id()).unwrap();
    let source = "https://registry.example.com/app.mpk".parse().unwrap();

    assert_refused(
        node.install_expected_bundle_blob(&expected, &blob_id, &source)
            .await,
    );
}

/// A group upgrade or activation marker can point a context at a blob that was
/// never installed, so the read the runtime is fed from must refuse it too.
#[tokio::test]
async fn a_dev_signed_blob_never_reaches_the_runtime() {
    let dir = TempDir::new().unwrap();
    let (node, _store, _d, _b) = node_client().await;
    let blob_id = store(&node, &dev_bundle(&dir)).await;

    assert_refused(node.application_bytes_from_blob(&blob_id, None).await);
}
