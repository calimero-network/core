//! One contract for every path that acquires application bytes: a mismatch is
//! rejected AND leaves nothing behind.

use actix::{Actor, Context, Handler};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::content_hash::ContentHash;
use calimero_primitives::context::ContextId;
use calimero_utils_actix::LazyRecipient;
use camino::Utf8PathBuf;
use libp2p::PeerId;
use tempfile::TempDir;

mod common;

const BYTES: &[u8] = b"some bytes";

/// Any syntactically valid peer id; the fake below never dials it.
const PEER: &str = "12D3KooWR5V4zmisVtVdGE6i8jfFwtgRNq5t8eDGxfckKuhXu7Eh";

/// A peer that advertises every blob and then serves bytes that hash to
/// something else, the way a hostile provider record would.
struct LyingPeer {
    peer_id: PeerId,
}

impl Actor for LyingPeer {
    type Context = Context<Self>;
}

impl Handler<NetworkMessage> for LyingPeer {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, _ctx: &mut Context<Self>) -> Self::Result {
        match msg {
            NetworkMessage::QueryBlob { outcome, .. } => {
                let _ignored = outcome.send(Ok(vec![self.peer_id]));
            }
            NetworkMessage::RequestBlob { outcome, .. } => {
                let _ignored = outcome.send(Ok(Some(BYTES.to_vec())));
            }
            _ => {}
        }
    }
}

#[actix::test]
async fn bytes_from_a_lying_peer_leave_nothing_behind() {
    let peer_id = PEER.parse::<PeerId>().expect("peer id");
    let network = LazyRecipient::new();
    let _peer = Actor::create(|ctx| {
        assert!(network.init(ctx));
        LyingPeer { peer_id }
    });

    let (node_client, _data, _blobs) =
        common::create_test_node_client_with(None, NetworkClient::new(network)).await;

    let wanted = BlobId::from([0xEE; 32]);
    let fetched = node_client
        .get_blob(&wanted, Some(&ContextId::from([0x11; 32])))
        .await
        .expect("network fetch");

    assert!(
        fetched.is_none(),
        "bytes that hash wrong must not be served"
    );
    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "the rejected bytes must be deleted, not left to accumulate once per \
         retry - there is no content-addressed GC to reclaim them"
    );
}

#[tokio::test]
async fn a_rejected_blob_leaves_no_bytes_behind() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (stored, _size) = node_client
        .add_blob(BYTES, Some(BYTES.len() as u64), None)
        .await
        .expect("store");
    let wrong = BlobId::from([0xEE; 32]);

    let err = node_client
        .verify_stored_blob(stored, Some(wrong))
        .await
        .expect_err("a mismatch must be rejected");

    assert!(err.to_string().contains("blob id mismatch"), "got: {err}");
    assert!(!node_client.has_blob(&stored).expect("lookup"));
}

#[tokio::test]
async fn a_matching_blob_is_kept() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (stored, _size) = node_client
        .add_blob(BYTES, Some(BYTES.len() as u64), None)
        .await
        .expect("store");

    node_client
        .verify_stored_blob(stored, Some(stored))
        .await
        .expect("a match must pass");
    assert!(node_client.has_blob(&stored).expect("lookup"));
}

// No expectation means no check: a path that legitimately has nothing to
// compare against (a local file install) must not lose its bytes.
#[tokio::test]
async fn no_expectation_keeps_the_blob() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let (stored, _size) = node_client
        .add_blob(BYTES, Some(BYTES.len() as u64), None)
        .await
        .expect("store");

    node_client
        .verify_stored_blob(stored, None)
        .await
        .expect("no expectation must pass");
    assert!(node_client.has_blob(&stored).expect("lookup"));
}

// The one place a human-supplied content digest is checked. It rejects, but it
// rejected *after* storing, so every wrong `?hash=` grew the blobstore.
#[tokio::test]
async fn a_wrong_content_hash_leaves_no_bytes_behind() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;

    let err = node_client
        .add_blob(
            BYTES,
            Some(BYTES.len() as u64),
            Some(&ContentHash::from([0xEE; 32])),
        )
        .await
        .expect_err("a wrong content hash must be rejected");

    assert!(err.to_string().contains("hash mismatch"), "got: {err}");
    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "the rejected bytes must be deleted, not left behind"
    );
}

// `expected_size` asserts a length the caller already knows, so it is never a
// ceiling: passing one as a cap rejects every correct blob under it.
#[tokio::test]
async fn a_wrong_advertised_size_leaves_no_bytes_behind() {
    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;

    let err = node_client
        .add_blob(BYTES, Some(BYTES.len() as u64 + 1), None)
        .await
        .expect_err("a wrong advertised size must be rejected");

    assert!(err.to_string().contains("size mismatch"), "got: {err}");
    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "the rejected bytes must be deleted, not left behind"
    );
}

// The same rule on the install that follows the store: a `.mpk` whose manifest
// will not verify is stored before it is opened, so its bytes must go too.
#[tokio::test]
async fn a_malformed_bundle_leaves_no_bytes_behind() {
    let dir = TempDir::new().expect("temp dir");
    let _bytes = common::pack_entries(&dir, "broken.mpk", &[("manifest.json", &b"{}"[..])]);
    let path: Utf8PathBuf = dir
        .path()
        .join("broken.mpk")
        .try_into()
        .expect("utf-8 path");

    let (node_client, _data, _blobs) = common::create_test_node_client(None).await;
    let err = node_client
        .install_application_from_path(path)
        .await
        .expect_err("an unverifiable bundle must be refused");

    assert!(
        node_client.list_blobs().expect("list").is_empty(),
        "a refused install must leave no bytes behind, got: {err}"
    );
}
