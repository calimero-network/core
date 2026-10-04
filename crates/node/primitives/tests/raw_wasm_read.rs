//! Raw wasm never reads as executable bytes: a group can name any blob this
//! node holds, and only a signed bundle verifies.

use calimero_node_primitives::test_fixtures::{node_client, signed_wasm};
use calimero_primitives::blobs::BlobId;
use calimero_store::{key, types};

const RAW: &[u8] = b"raw wasm, not a bundle";
const NAMED: [u8; 32] = [0x5A; 32]; // the application id governance named

/// The stub `ContextRegistered` seeds: it names the blob and holds no bytes.
fn stub(blob_id: BlobId) -> types::ApplicationMeta {
    types::ApplicationMeta::new(
        key::BlobMeta::new(blob_id),
        0,
        "calimero://pending-blob-share".into(),
        Box::default(),
        key::BlobMeta::new(BlobId::from([0; 32])),
        types::PackageInfo {
            package: "".into(),
            version: "".into(),
            signer_id: "".into(),
            state_version: 0,
        },
    )
}

/// A group target or activation marker reads its blob directly, and a stub row
/// reads it through the row; neither may run raw bytes no install bound.
#[tokio::test]
async fn a_held_raw_blob_only_a_group_named_never_reads_as_wasm() {
    let (node, store, _data, _blobs) = node_client().await;
    let (blob_id, _size) = node
        .add_blob(RAW, Some(RAW.len() as u64), None)
        .await
        .expect("store the bytes");

    let _refused = node
        .application_bytes_from_blob(&blob_id, None)
        .await
        .expect_err("raw bytes no row holds must not read as wasm");

    store
        .handle()
        .put(&key::ApplicationMeta::new(NAMED.into()), &stub(blob_id))
        .expect("seed the stub");
    let _refused = node
        .application_bytes_from_blob(&blob_id, None)
        .await
        .expect_err("a stub names bytes, it does not install them");
    let _refused = node
        .get_application_bytes(&NAMED.into(), None)
        .await
        .expect_err("the row path reads the same gate");
}

/// A raw row holding its bytes is what the old remote install left behind, and
/// it is indistinguishable from any other: raw wasm never reads as wasm.
#[tokio::test]
async fn a_raw_row_holding_its_bytes_never_reads_as_wasm() {
    let (node, store, _data, _blobs) = node_client().await;
    let (blob_id, size) = node
        .add_blob(RAW, Some(RAW.len() as u64), None)
        .await
        .expect("store the bytes");
    let mut row = stub(blob_id);
    row.size = size;
    store
        .handle()
        .put(&key::ApplicationMeta::new(NAMED.into()), &row)
        .expect("a raw row");

    let _refused = node
        .get_application_bytes(&NAMED.into(), None)
        .await
        .expect_err("raw wasm must not read as wasm");
}

/// Control: a signed bundle's wasm reads through the same gate.
#[tokio::test]
async fn a_signed_bundle_still_reads_as_wasm() {
    let (node, _store, _data, _blobs) = node_client().await;
    let bundle = signed_wasm(RAW);
    let (blob_id, _size) = node
        .add_blob(bundle.as_slice(), Some(bundle.len() as u64), None)
        .await
        .expect("store the bundle");

    let wasm = node
        .application_bytes_from_blob(&blob_id, None)
        .await
        .expect("read")
        .expect("held");
    assert_eq!(&*wasm, RAW);
}
