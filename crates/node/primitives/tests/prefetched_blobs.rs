//! A prefetch's reference to a blob is its own: releasing it frees bytes only the
//! prefetch held, never a reference another owner took.

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use common::create_test_node_client;

mod common;

const BYTES: &[u8] = b"a file a fleet node prefetched";
const CONTEXT: [u8; 32] = [0x11; 32];
const OTHER: [u8; 32] = [0x22; 32];

async fn add(node_client: &NodeClient, bytes: &[u8]) -> BlobId {
    let (blob, _size) = node_client
        .add_blob(bytes, Some(bytes.len() as u64), None)
        .await
        .expect("store bytes");
    blob
}

/// The prefetch's own add of `bytes` for `context`.
async fn prefetch(node_client: &NodeClient, bytes: &[u8], context: [u8; 32]) -> BlobId {
    let blob = add(node_client, bytes).await;
    node_client
        .record_prefetched_blob(&ContextId::from(context), &blob)
        .await
        .expect("record the prefetch");
    blob
}

async fn release(node_client: &NodeClient, context: [u8; 32]) {
    node_client
        .release_prefetched_blobs(&ContextId::from(context))
        .await
        .expect("release the prefetches");
}

#[actix::test]
async fn releasing_a_contexts_prefetches_frees_bytes_only_they_held() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let blob = prefetch(&node_client, BYTES, CONTEXT).await;

    release(&node_client, CONTEXT).await;

    assert!(!node_client.has_blob(&blob).expect("read"));
    assert!(node_client
        .prefetched_blob_contexts()
        .expect("read")
        .is_empty());
}

#[actix::test]
async fn releasing_a_prefetch_keeps_bytes_another_owner_holds() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let uploaded = add(&node_client, BYTES).await;
    let _prefetched = prefetch(&node_client, BYTES, CONTEXT).await;

    release(&node_client, CONTEXT).await;
    assert!(
        node_client.has_blob(&uploaded).expect("read"),
        "the upload's reference stays"
    );

    let _freed = node_client
        .delete_blob(uploaded)
        .await
        .expect("release the upload");
    assert!(
        !node_client.has_blob(&uploaded).expect("read"),
        "no reference was left over"
    );
}

/// A second prefetch of a blob the context already holds keeps one reference.
#[actix::test]
async fn a_repeated_prefetch_of_one_blob_holds_one_reference() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let blob = prefetch(&node_client, BYTES, CONTEXT).await;
    let _again = prefetch(&node_client, BYTES, CONTEXT).await;

    release(&node_client, CONTEXT).await;

    assert!(!node_client.has_blob(&blob).expect("read"));
}

#[actix::test]
async fn releasing_one_contexts_prefetches_keeps_anothers() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let blob = prefetch(&node_client, BYTES, CONTEXT).await;
    let _other = prefetch(&node_client, BYTES, OTHER).await;
    assert_eq!(
        node_client.prefetched_blob_contexts().expect("read"),
        [ContextId::from(CONTEXT), ContextId::from(OTHER)].into(),
    );

    release(&node_client, CONTEXT).await;
    assert!(
        node_client.has_blob(&blob).expect("read"),
        "the other context's prefetch stays"
    );

    release(&node_client, OTHER).await;
    assert!(!node_client.has_blob(&blob).expect("read"));
}

/// Once a blob is freed by another path, its prefetch rows go with it, so the
/// same bytes stored again by someone else are not released in the prefetch's name.
#[actix::test]
async fn a_freed_blob_forgets_its_prefetches() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let blob = prefetch(&node_client, BYTES, CONTEXT).await;
    let _freed = node_client.delete_blob(blob).await.expect("free the blob");
    let reuploaded = add(&node_client, BYTES).await;

    release(&node_client, CONTEXT).await;

    assert!(node_client.has_blob(&reuploaded).expect("read"));
}

/// A prefetch whose blob was freed meanwhile has nothing to hold, so it leaves no
/// row to release the same bytes stored again by someone else.
#[actix::test]
async fn a_prefetch_recorded_after_its_blob_was_freed_records_nothing() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let blob = add(&node_client, BYTES).await;
    let _freed = node_client.delete_blob(blob).await.expect("free the blob");
    node_client
        .record_prefetched_blob(&ContextId::from(CONTEXT), &blob)
        .await
        .expect("record the prefetch");
    let uploaded = add(&node_client, BYTES).await;

    release(&node_client, CONTEXT).await;

    assert!(node_client.has_blob(&uploaded).expect("read"));
    assert!(node_client
        .prefetched_blob_contexts()
        .expect("read")
        .is_empty());
}
