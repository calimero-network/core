//! `add_blob`'s size argument is an exact assertion, not a stream ceiling.
//!
//! Sync's blob share once passed its rogue-sender ceiling here, so every
//! transfer whose advertised size was unknown stored the blob correctly and
//! then failed on the length check, leaving the receiver on a size-0 stub.

use calimero_node_primitives::test_fixtures::node_client;
use futures_util::io::Cursor;
use futures_util::AsyncReadExt;

const PAYLOAD: &[u8] = b"a bundle far shorter than the stream ceiling";
const CEILING: u64 = 500 * 1024 * 1024;

#[tokio::test]
async fn a_ceiling_passed_as_the_expected_size_rejects_a_shorter_blob() {
    let (node, _store, _data, _blobs) = node_client().await;

    let err = node
        .add_blob(Cursor::new(PAYLOAD), Some(CEILING), None)
        .await
        .expect_err("the size argument is exact, so a ceiling must not be passed as one");

    assert!(
        err.to_string().contains("size mismatch"),
        "expected a size mismatch, got: {err}"
    );
}

#[tokio::test]
async fn a_capped_reader_stores_a_shorter_blob_at_its_true_size() {
    let (node, _store, _data, _blobs) = node_client().await;

    let (_blob_id, size) = node
        .add_blob(Cursor::new(PAYLOAD).take(CEILING), None, None)
        .await
        .expect("a capped reader bounds the transfer without asserting a length");

    assert_eq!(size, PAYLOAD.len() as u64);
}
