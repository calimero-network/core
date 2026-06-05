//! Regression: `add_local_applied_delta` must prune non-head ancestors
//! from `head_root_hashes`, matching `add_delta_internal`'s `retain(...)`
//! trimming at the end of its own flow.

use std::sync::Arc;

use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context_client::client::ContextClient;
use calimero_dag::{CausalDelta, DeltaKind};
use calimero_network_primitives::client::NetworkClient;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;
use calimero_utils_actix::LazyRecipient;
use tokio::sync::{broadcast, mpsc};

use crate::delta_store::DeltaStore;

fn make_delta(
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    expected_root_hash: [u8; 32],
) -> CausalDelta<Vec<Action>> {
    CausalDelta {
        id,
        parents,
        payload: Vec::new(),
        hlc: HybridTimestamp::default(),
        expected_root_hash,
        kind: DeltaKind::Regular,
    }
}

#[tokio::test]
async fn add_local_applied_delta_prunes_non_head_ancestors() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Store::new(Arc::new(InMemoryDB::owned()));

    let blob_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store);

    let node_recipient = LazyRecipient::new();
    let context_recipient = LazyRecipient::new();
    let network_recipient = LazyRecipient::new();

    let network_client = NetworkClient::new(network_recipient);
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, _ctx_sync_rx) = mpsc::channel(1);
    let (ns_sync_tx, _ns_sync_rx) = mpsc::channel(1);
    let (ns_join_tx, _ns_join_rx) = mpsc::channel(1);
    let (open_subgroup_join_tx, _open_subgroup_join_rx) = mpsc::channel(1);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);

    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client,
        node_recipient,
        event_sender,
        sync_client,
        String::new(),
        None,
    );

    let context_client = ContextClient::new(store, node_client, context_recipient);

    let context_id = ContextId::from([0xAA; 32]);
    let our_identity = PublicKey::from([0xBB; 32]);
    let root = [0u8; 32];

    let delta_store = DeltaStore::new(root, context_client, context_id, our_identity);

    // Register a "parent" delta (genesis as its parent).
    let parent_id = [0x11u8; 32];
    let parent_hash = [0xA1; 32];
    let cascaded = delta_store
        .add_local_applied_delta(make_delta(parent_id, vec![[0u8; 32]], parent_hash))
        .await
        .expect("add parent succeeds");
    assert!(
        cascaded.is_empty(),
        "no pending children → no cascaded events"
    );

    let ids = delta_store.head_root_hash_ids().await;
    assert_eq!(
        ids,
        vec![parent_id],
        "parent is the sole head after first add"
    );

    // Register a child of the parent. Parent is no longer a head and
    // should be pruned from `head_root_hashes` so stale lookups don't
    // return its root hash.
    let child_id = [0x22u8; 32];
    let child_hash = [0xC1; 32];
    let cascaded = delta_store
        .add_local_applied_delta(make_delta(child_id, vec![parent_id], child_hash))
        .await
        .expect("add child");
    assert!(cascaded.is_empty(), "still no pending children to cascade");

    let ids = delta_store.head_root_hash_ids().await;
    assert_eq!(
        ids,
        vec![child_id],
        "parent entry must be pruned once it is no longer a head"
    );
}

/// P4: a locally-created delta must self-log its own `Shared` rotations into
/// the entity's rotation log (the receive path does this via
/// `maybe_append_rotation_log`; the originator has no such path). Genesis Add
/// and writer-set changes append entries; a plain value-write does not.
#[tokio::test]
async fn add_local_applied_delta_self_logs_own_rotations() {
    use std::collections::BTreeSet;

    use calimero_storage::address::Id;
    use calimero_storage::tests::common::{build_signed_shared_action, pubkey_of};
    use ed25519_dalek::SigningKey;

    use crate::delta_store::load_rotation_log_direct;

    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let blob_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_config).await.expect("blob fs");
    let blob_store = BlobStore::new(store.clone(), file_system);
    let blob_manager = BlobManager::new(blob_store);
    let network_client = NetworkClient::new(LazyRecipient::new());
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, _r0) = mpsc::channel(1);
    let (ns_sync_tx, _r1) = mpsc::channel(1);
    let (ns_join_tx, _r2) = mpsc::channel(1);
    let (open_subgroup_join_tx, _r3) = mpsc::channel(1);
    let sync_client = SyncClient::new(ctx_sync_tx, ns_sync_tx, ns_join_tx, open_subgroup_join_tx);
    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client,
        LazyRecipient::new(),
        event_sender,
        sync_client,
        String::new(),
        None,
    );
    let context_id = ContextId::from([0xAA; 32]);
    let our_identity = PublicKey::from([0xBB; 32]);
    // Keep a clone to read the rotation log back; `DeltaStore::new` takes one.
    let context_client = ContextClient::new(store, node_client, LazyRecipient::new());
    let reader = context_client.clone();
    let delta_store = DeltaStore::new([0u8; 32], context_client, context_id, our_identity);

    let anchor = Id::new([0x33; 32]);
    let alice_sk = SigningKey::from_bytes(&[0xA1; 32]);
    let bob_sk = SigningKey::from_bytes(&[0xB2; 32]);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let writers = |set: &[PublicKey]| -> BTreeSet<PublicKey> { set.iter().copied().collect() };

    // Build a signed Shared Add/Update for `anchor` carrying `w` as its writer
    // set (signed by alice; the self-log only reads writers/signer, not verify).
    let shared_action = |add: bool, w: BTreeSet<PublicKey>, nonce: u64| -> Action {
        build_signed_shared_action(add, anchor, vec![1], w, nonce, &alice_sk, vec![])
    };

    let log_entries = || {
        load_rotation_log_direct(&reader, context_id, anchor)
            .expect("read log")
            .map(|l| {
                l.entries
                    .iter()
                    .map(|e| e.new_writers.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };

    // 1. Genesis Add {alice} → bootstrap entry recorded.
    let mut d = make_delta([0x01; 32], vec![[0u8; 32]], [0xD1; 32]);
    d.payload = vec![shared_action(true, writers(&[alice]), 1)];
    let _ = delta_store
        .add_local_applied_delta(d)
        .await
        .expect("add genesis");
    assert_eq!(
        log_entries(),
        vec![calimero_storage::entities::full_mask(writers(&[alice]))],
        "genesis Add must self-log a bootstrap entry"
    );

    // 2. Rotation Update {alice,bob} → second entry recorded.
    let mut d = make_delta([0x02; 32], vec![[0x01; 32]], [0xD2; 32]);
    d.payload = vec![shared_action(false, writers(&[alice, bob]), 2)];
    let _ = delta_store
        .add_local_applied_delta(d)
        .await
        .expect("add rotation");
    assert_eq!(
        log_entries(),
        vec![
            calimero_storage::entities::full_mask(writers(&[alice])),
            calimero_storage::entities::full_mask(writers(&[alice, bob]))
        ],
        "writer-set change must self-log a rotation entry"
    );

    // 3. Value-write Update with UNCHANGED writers {alice,bob} → no new entry.
    let mut d = make_delta([0x03; 32], vec![[0x02; 32]], [0xD3; 32]);
    d.payload = vec![shared_action(false, writers(&[alice, bob]), 3)];
    let _ = delta_store
        .add_local_applied_delta(d)
        .await
        .expect("add value-write");
    assert_eq!(
        log_entries(),
        vec![
            calimero_storage::entities::full_mask(writers(&[alice])),
            calimero_storage::entities::full_mask(writers(&[alice, bob]))
        ],
        "a value-write that doesn't change writers must NOT append a rotation entry"
    );
}
