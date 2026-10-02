//! A blob is held for a context only once its bytes came from a peer that
//! served them under that context; a local hit or a lying peer records nothing.

use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use common::{
    blob_id_of, create_test_node_client, create_test_node_client_with, fake_peer_network,
    PeerBehavior,
};

mod common;

const BYTES: &[u8] = b"a file a context member shared";
const CONTEXT: [u8; 32] = [0x11; 32];
const OTHER: [u8; 32] = [0x22; 32];

#[actix::test]
async fn a_blob_fetched_for_a_context_is_held_for_it() {
    let (network, _peer) = fake_peer_network(PeerBehavior::Serves(BYTES.to_vec()));
    let (node_client, _data, _blobs) = create_test_node_client_with(None, network).await;
    let wanted = blob_id_of(BYTES).await;

    let fetched = node_client
        .get_blob(&wanted, Some(&ContextId::from(CONTEXT)))
        .await
        .expect("network fetch");

    assert!(fetched.is_some(), "the peer serves the blob");
    assert!(node_client
        .is_blob_held_for_context(&ContextId::from(CONTEXT), &wanted)
        .unwrap());
    assert!(!node_client
        .is_blob_held_for_context(&ContextId::from(OTHER), &wanted)
        .unwrap());
}

#[actix::test]
async fn bytes_served_under_the_wrong_id_are_held_for_nothing() {
    let (network, _peer) = fake_peer_network(PeerBehavior::Serves(BYTES.to_vec()));
    let (node_client, _data, _blobs) = create_test_node_client_with(None, network).await;

    let fetched = node_client
        .get_blob(&BlobId::from([0xEE; 32]), Some(&ContextId::from(CONTEXT)))
        .await
        .expect("network fetch");

    assert!(fetched.is_none());
    assert!(!node_client
        .is_blob_held_for_context(&ContextId::from(CONTEXT), &blob_id_of(BYTES).await)
        .unwrap());
}

/// Naming a context while reading a blob held for another must not claim it.
#[actix::test]
async fn a_local_hit_records_no_context() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let (local, _size) = node_client
        .add_blob(BYTES, Some(BYTES.len() as u64), None)
        .await
        .expect("store bytes");

    let fetched = node_client
        .get_blob(&local, Some(&ContextId::from(CONTEXT)))
        .await
        .expect("local read");

    assert!(fetched.is_some());
    assert!(!node_client
        .is_blob_held_for_context(&ContextId::from(CONTEXT), &local)
        .unwrap());
}

/// Once the last reference goes, so do the contexts the blob was held for:
/// identical bytes added again later with no context must not be served.
#[actix::test]
async fn freeing_a_blob_forgets_the_contexts_it_was_held_for() {
    let (node_client, _data, _blobs) = create_test_node_client(None).await;
    let context = ContextId::from(CONTEXT);
    let mut added = None;
    for _ in 0..2 {
        let (blob_id, _size) = node_client
            .add_blob(BYTES, Some(BYTES.len() as u64), None)
            .await
            .expect("store bytes");
        added = Some(blob_id);
    }
    let blob_id = added.unwrap();
    node_client.record_blob_owner(&context, &blob_id).unwrap();

    assert!(node_client.delete_blob(blob_id).await.unwrap());
    assert!(
        node_client
            .is_blob_held_for_context(&context, &blob_id)
            .unwrap(),
        "another reference still holds the bytes"
    );

    assert!(node_client.delete_blob(blob_id).await.unwrap());
    let _readded = node_client
        .add_blob(BYTES, Some(BYTES.len() as u64), None)
        .await
        .expect("store bytes again");
    assert!(!node_client
        .is_blob_held_for_context(&context, &blob_id)
        .unwrap());
}
