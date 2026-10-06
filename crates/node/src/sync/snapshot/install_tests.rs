//! The snapshot install, driven the way a joiner drives it: pages arrive over a
//! stream, and the store ends up holding the source's tree or, when the tree
//! does not hold together, exactly what it held before.

use std::cell::Cell;
use std::num::NonZeroU64;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use calimero_blobstore::config::BlobStoreConfig;
use calimero_blobstore::{BlobManager as BlobStore, FileSystem};
use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::test_fixtures::{enrol_member, sample_meta_with_admin};
use calimero_governance_store::unified_op_decode::op_from_namespace_op;
use calimero_governance_store::{
    register_context_in_group, AbsorbRepository, MetaRepository, NotFolded,
};
use calimero_governance_types::{EncryptedGroupOp, GroupOp, NamespaceOp, SignedNamespaceOp};
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::{BlobManager, NodeClient, SyncClient};
use calimero_node_primitives::messages::NodeMessage;
use calimero_node_primitives::sync::storage_bridge::create_runtime_env;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_storage::action::Action;
use calimero_storage::child_trie::ChildTrie;
use calimero_storage::collections::{Root, UnorderedMap, Vector};
use calimero_storage::entities::{ChildInfo, EntryRules, Metadata, SignatureData, StorageType};
use calimero_storage::env::{with_runtime_env, RuntimeEnv};
use calimero_storage::index::Index;
use calimero_storage::interface::ApplyContext;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_storage::tests::common::owned_entry_id;
use calimero_store::db::{Column, InMemoryDB};
use calimero_store::key::STATE_KEY_LEN;
use calimero_utils_actix::LazyRecipient;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::{broadcast, mpsc};

use super::leaf::rows::{decode, IndexRow};
use super::*;
use crate::sync::{SyncConfig, SyncManager};
use crate::NodeState;

const CONTEXT: [u8; 32] = [0x33; 32];
const STALE: [u8; STATE_KEY_LEN] = [0x77; STATE_KEY_LEN];
const NAMESPACE: [u8; 32] = [0x4A; 32];
const RELAY_SEATED: [u8; 32] = [0xE1; 32]; // id of the op that admits the relay
const RELAY_REMOVED: [u8; 32] = [0xE2; 32]; // id of the op that removes it

fn context() -> ContextId {
    ContextId::from(CONTEXT)
}

fn root() -> Id {
    Id::new(CONTEXT)
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// A tree the storage layer built: a root, four collections under it, and
/// three entries under the second collection.
fn source() -> (Store, Hash) {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let env = create_runtime_env(
        &store,
        context(),
        PublicKey::from([2; 32]),
        AccountId::from([0xAC; 32]),
    );
    with_runtime_env(env, || {
        let apply = |id: Id, data: Vec<u8>, parents: &[Id]| {
            let ancestors = parents
                .iter()
                .map(|parent| ChildInfo::new(*parent, [0; 32], Metadata::default()))
                .collect();
            Interface::<MainStorage>::apply_action(
                Action::Add {
                    id,
                    data,
                    ancestors,
                    metadata: Metadata::new(1, 1),
                },
                &ApplyContext::empty(),
            )
            .unwrap();
        };
        apply(root(), b"root".to_vec(), &[]);
        for n in 1..=4_u8 {
            apply(Id::new([n; 32]), vec![n; 40], &[root()]);
        }
        for n in 0x21..=0x23_u8 {
            apply(Id::new([n; 32]), vec![n; 17], &[Id::new([2; 32]), root()]);
        }
    });
    let hash = served_state_root(&store, context()).unwrap();
    assert_ne!(*hash, [0; 32]);
    (store, hash)
}

/// What the source would ship for its whole state.
fn shipped(store: &Store) -> Vec<SnapshotRecord> {
    let handle = store.handle();
    let mut out = Vec::new();
    let mut cursor = None;
    loop {
        let (pages, next, _) =
            generate_snapshot_pages(&handle, context(), cursor.as_ref(), 16, 64 * 1024, None)
                .unwrap();
        for page in &pages {
            out.extend(decode_snapshot_records(page).unwrap());
        }
        match next {
            Some(next) => cursor = Some(next),
            None => return out,
        }
    }
}

/// The `(entry, index, schema)` of the record for `wanted`, to be rewritten.
fn entity(
    records: &mut [SnapshotRecord],
    wanted: Id,
) -> (&mut Vec<u8>, &mut Vec<u8>, &mut Option<[u8; 32]>) {
    records
        .iter_mut()
        .find_map(|record| match record {
            SnapshotRecord::Entity {
                id,
                entry,
                index,
                schema_bytecode_id,
            } if *id == *wanted.as_bytes() => Some((entry, index, schema_bytecode_id)),
            _ => None,
        })
        .expect("the source ships that entity")
}

/// A joiner over an in-memory store, holding one identity of its own for the
/// context so it can ask for a snapshot.
struct Joiner {
    manager: SyncManager,
    store: Store,
    _tmp: TempDir,
}

async fn joiner() -> Joiner {
    let tmp = tempfile::tempdir().expect("tempdir");
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let blob_store_config =
        BlobStoreConfig::new(tmp.path().to_path_buf().try_into().expect("utf8 blob path"));
    let file_system = FileSystem::new(&blob_store_config).await.expect("blob fs");
    let blob_manager = BlobManager::new(BlobStore::new(store.clone(), file_system));

    let network_client = NetworkClient::new(LazyRecipient::new());
    let (event_sender, _) = broadcast::channel(16);
    let (ctx_sync_tx, ctx_sync_rx) = mpsc::channel(64);
    let (ns_sync_tx, ns_sync_rx) = mpsc::channel(16);
    let (ns_join_tx, ns_join_rx) = mpsc::channel(16);
    let (open_subgroup_join_tx, open_subgroup_join_rx) = mpsc::channel(16);
    let (relay_sealed_join_tx, relay_sealed_join_rx) = mpsc::channel(16);
    let sync_client = SyncClient::new(
        ctx_sync_tx,
        ns_sync_tx,
        ns_join_tx,
        open_subgroup_join_tx,
        relay_sealed_join_tx,
    );
    let node_client = NodeClient::new(
        store.clone(),
        blob_manager,
        network_client.clone(),
        LazyRecipient::<NodeMessage>::new(),
        event_sender,
        sync_client,
        None,
    );
    let context_client =
        ContextClient::new(store.clone(), node_client.clone(), LazyRecipient::new());
    let manager = SyncManager::new(
        SyncConfig::default(),
        node_client,
        context_client,
        network_client,
        NodeState::new(),
        ctx_sync_rx,
        ns_sync_rx,
        ns_join_rx,
        open_subgroup_join_rx,
        relay_sealed_join_rx,
    );

    // No network actor stands behind the manager, so the peer id it signs its
    // request under is given rather than asked for.
    manager
        .local_peer_id
        .set(libp2p::PeerId::random())
        .expect("not set yet");

    let own = PrivateKey::from([0x21; 32]);
    store
        .handle()
        .put(
            &calimero_store::key::ContextIdentity::new(context(), own.public_key()),
            &calimero_store::types::ContextIdentity {
                private_key: Some(*own.as_bytes()),
            },
        )
        .expect("identity");
    Joiner {
        manager,
        store,
        _tmp: tmp,
    }
}

impl Joiner {
    /// A key the joiner held before the snapshot, which the snapshot does not
    /// carry.
    fn holds_stale_state(&self) {
        self.store
            .handle()
            .put(
                &ContextStateKey::new(context(), STALE),
                &ContextStateValue::from(Slice::from(b"before".to_vec())),
            )
            .unwrap();
    }

    fn keys(&self) -> Vec<[u8; STATE_KEY_LEN]> {
        let handle = self.store.handle();
        let mut iter = handle.iter::<ContextStateKey>().unwrap();
        let mut keys: Vec<[u8; STATE_KEY_LEN]> = iter
            .entries()
            .map(|(key, _)| key.unwrap())
            .filter(|key| key.context_id() == context())
            .map(|key| key.state_key())
            .collect();
        keys.sort();
        keys
    }

    /// Whether the staging area holds nothing for the context.
    fn stages_nothing(&self) -> bool {
        let prefix = context();
        let hi = [prefix.as_ref(), &[0xFF; STATE_KEY_LEN + 1][..]].concat();
        self.store
            .raw_scan(Column::SnapshotStage, prefix.as_ref(), &hi, Some(1))
            .unwrap()
            .is_empty()
    }

    fn holds(&self, id: Id) -> bool {
        self.keys().contains(&StorageKey::Index(id).to_bytes())
    }

    /// The joiner's install of `records` under the boundary root `claimed`.
    async fn installs(
        &self,
        claimed: Hash,
        records: Vec<SnapshotRecord>,
    ) -> Result<(usize, Option<[u8; 32]>)> {
        self.installs_in_pages(claimed, vec![records]).await
    }

    /// As [`installs`](Self::installs), one page per request, resumed by cursor.
    async fn installs_in_pages(
        &self,
        claimed: Hash,
        pages: Vec<Vec<SnapshotRecord>>,
    ) -> Result<(usize, Option<[u8; 32]>)> {
        let (mut ours, theirs) = Stream::test_pair();
        let server = tokio::spawn(serve(theirs, pages));
        let boundary = SnapshotBoundary {
            boundary_timestamp: 0,
            boundary_root_hash: claimed,
            dag_heads: Vec::new(),
        };
        let outcome = self
            .manager
            .request_and_apply_snapshot_pages(context(), &boundary, false, &mut ours)
            .await;
        server.await.expect("the source finished");
        outcome
    }
}

/// Answer each page request with the next of `pages`, the last without a cursor.
async fn serve(mut end: Stream, pages: Vec<Vec<SnapshotRecord>>) {
    let count = pages.len();
    for (n, records) in pages.into_iter().enumerate() {
        let _request = crate::sync::stream::recv(&mut end, None, Duration::from_secs(5))
            .await
            .expect("the source receives the request");
        let (payload, uncompressed_len) = if records.is_empty() {
            (Vec::new(), 0)
        } else {
            let mut page = vec![SNAPSHOT_PAGE_FORMAT_V2];
            for record in &records {
                page.extend(encode_framed_snapshot_record(record).unwrap());
            }
            (
                lz4_flex::compress_prepend_size(&page),
                u32::try_from(page.len()).unwrap(),
            )
        };
        let reply = StreamMessage::Message {
            sequence_id: n as u64,
            payload: MessagePayload::SnapshotPage {
                payload: payload.into(),
                uncompressed_len,
                cursor: (n + 1 < count).then(|| vec![n as u8]),
                page_count: 1,
                sent_count: 1,
                total_records: records.len() as u64,
            },
            next_nonce: [0; 12],
        };
        crate::sync::stream::send(&mut end, &reply, None)
            .await
            .expect("the source replies");
    }
}

#[tokio::test]
async fn a_snapshot_is_installed_over_what_it_replaces() {
    let (source, claimed) = source();
    let joiner = joiner().await;
    joiner.holds_stale_state();

    let (applied, _) = joiner.installs(claimed, shipped(&source)).await.unwrap();

    assert_eq!(applied, 8);
    assert!(!joiner.keys().contains(&STALE), "what it replaces is gone");
    // One row per entity, holding its index record and its data.
    assert_eq!(joiner.keys().len(), 8);
    assert_eq!(
        served_state_root(&joiner.store, context()).unwrap(),
        claimed
    );
}

#[derive(BorshSerialize, BorshDeserialize)]
struct App {
    names: UnorderedMap<String, String>,
    log: Vector<String>,
}

/// The state an app writes through the collections, some of it since deleted.
fn app_source() -> (Store, Hash) {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let env = create_runtime_env(
        &store,
        context(),
        PublicKey::from([2; 32]),
        AccountId::from([0xAC; 32]),
    );
    with_runtime_env(env, || {
        let mut app = Root::new(|| App {
            names: UnorderedMap::new(),
            log: Vector::new(),
        });
        for n in 0..40 {
            app.names.insert(format!("k{n}"), format!("v{n}")).unwrap();
        }
        for n in 0..10 {
            app.log.push(format!("l{n}")).unwrap();
        }
        app.commit();

        let mut app = Root::<App>::fetch().unwrap();
        for n in 0..10 {
            app.names.remove(&format!("k{}", n * 3)).unwrap();
        }
        app.commit();
    });
    let hash = served_state_root(&store, context()).unwrap();
    assert_ne!(*hash, [0; 32]);
    (store, hash)
}

#[tokio::test]
async fn the_state_of_an_app_written_through_its_collections_passes_the_entity_checks() {
    let (source, claimed) = app_source();
    let records = shipped(&source);
    assert!(records.len() > 40, "{} records", records.len());
    let joiner = joiner().await;

    let (applied, _) = joiner.installs(claimed, records.clone()).await.unwrap();

    assert_eq!(applied, records.len());
    assert_eq!(
        served_state_root(&joiner.store, context()).unwrap(),
        claimed
    );
}

#[tokio::test]
async fn a_leaf_whose_bytes_do_not_match_its_own_hash_is_not_installed() {
    let (source, claimed) = source();
    let leaf = Id::new([0x21; 32]);
    let mut records = shipped(&source);
    entity(&mut records, leaf).0.push(1);
    let joiner = joiner().await;

    let outcome = joiner.installs(claimed, records).await;

    assert!(
        outcome.is_err(),
        "installed a leaf that is not its own hash"
    );
    assert!(!joiner.holds(leaf));
}

#[tokio::test]
async fn a_leaf_dated_ahead_of_the_clock_is_not_installed() {
    let (source, claimed) = source();
    let leaf = Id::new([0x22; 32]);
    let mut records = shipped(&source);
    let (_, index, _) = entity(&mut records, leaf);
    let mut dated = decode(index);
    dated.metadata = Metadata::new(1, u64::MAX / 2);
    *index = dated.encode();
    let joiner = joiner().await;

    let outcome = joiner.installs(claimed, records).await;

    assert!(outcome.is_err(), "installed a leaf dated in the future");
    assert!(!joiner.holds(leaf));
}

#[tokio::test]
async fn an_empty_snapshot_that_claims_a_root_does_not_clear_the_context() {
    let (_, claimed) = source();
    let joiner = joiner().await;
    joiner.holds_stale_state();

    let outcome = joiner.installs(claimed, Vec::new()).await;

    assert!(
        outcome.is_err(),
        "cleared a context for a root it never sent"
    );
    assert_eq!(joiner.keys(), vec![STALE]);
}

#[tokio::test]
async fn an_empty_snapshot_of_an_empty_context_clears_the_context() {
    let joiner = joiner().await;
    joiner.holds_stale_state();

    joiner
        .installs(Hash::from([0; 32]), Vec::new())
        .await
        .unwrap();

    assert!(joiner.keys().is_empty());
}

/// A leaf stamped for a schema the joiner cannot read yet is held in the absorb
/// buffer, and only once the tree it belongs to has checked out.
#[tokio::test]
async fn a_leaf_declined_for_its_schema_is_buffered_only_once_the_snapshot_verifies() {
    let (source, claimed) = source();
    let declined = Id::new([0x23; 32]);
    let mut records = shipped(&source);
    *entity(&mut records, declined).2 = Some([2; 32]);

    let good = joiner().await;
    calimero_context::activation::record_activation(&good.store, &context(), [1; 32]);
    let (applied, _) = good.installs(claimed, records.clone()).await.unwrap();
    assert_eq!(applied, 7);
    assert!(!good.holds(declined), "an unreadable leaf is not stored");
    let buffered = AbsorbRepository::new(&good.store)
        .enumerate_pending(&context())
        .unwrap();
    assert_eq!(buffered.len(), 1);

    let bad = joiner().await;
    calimero_context::activation::record_activation(&bad.store, &context(), [1; 32]);
    let outcome = bad.installs(Hash::from([0xAB; 32]), records).await;
    assert!(outcome.is_err());
    let buffered = AbsorbRepository::new(&bad.store)
        .enumerate_pending(&context())
        .unwrap();
    assert!(buffered.is_empty(), "buffered the leaves of a refused tree");
}

#[tokio::test]
async fn an_installed_snapshot_carries_the_sync_marker_until_it_is_finalised() {
    let (source, claimed) = source();
    let joiner = joiner().await;

    joiner.installs(claimed, shipped(&source)).await.unwrap();

    assert_eq!(
        joiner.manager.check_sync_in_progress(context()).unwrap(),
        Some(claimed)
    );
}

#[tokio::test]
async fn a_tree_served_over_several_pages_is_installed_once_the_last_arrives() {
    let (source, claimed) = source();
    let records = shipped(&source);
    let (first, second) = records.split_at(4);
    let joiner = joiner().await;
    joiner.holds_stale_state();

    let (applied, _) = joiner
        .installs_in_pages(claimed, vec![first.to_vec(), second.to_vec()])
        .await
        .unwrap();

    assert_eq!(applied, 8);
    assert!(!joiner.keys().contains(&STALE));
    assert_eq!(
        served_state_root(&joiner.store, context()).unwrap(),
        claimed
    );
}

/// A child of the root: its index row agrees with `indexed`, and it ships `shipped`.
fn child(
    id: Id,
    indexed: &[u8],
    shipped: &[u8],
    storage_type: StorageType,
) -> (Id, Vec<u8>, IndexRow) {
    let own_hash = sha(indexed);
    let mut metadata = Metadata::new(1, 1);
    metadata.storage_type = storage_type;
    let row = IndexRow {
        id: *id.as_bytes(),
        parent_id: Some(*root().as_bytes()),
        full_hash: sha(&own_hash),
        own_hash,
        metadata,
        deleted_at: None,
        deleted_children: Vec::new(),
    };
    (id, shipped.to_vec(), row)
}

/// A root over `children`, and the hash the root folds to.
fn tree_over(children: &[(Id, Vec<u8>, IndexRow)]) -> (Vec<SnapshotRecord>, Hash) {
    let mut rows: BTreeMap<StorageKey, Vec<u8>> = BTreeMap::new();
    for (id, _, row) in children {
        let mut writes = Vec::new();
        ChildTrie::<MainStorage>::insert_with(
            root(),
            ChildInfo::new(*id, row.full_hash, row.metadata.clone()),
            |key| rows.get(&key).cloned(),
            |key, bytes| writes.push((key, bytes.to_vec())),
        );
        rows.extend(writes);
    }
    let root_entry = b"root".to_vec();
    let root_own = sha(&root_entry);
    let root_full =
        Index::<MainStorage>::full_hash_with(root(), root_own, |key| rows.get(&key).cloned())
            .unwrap();
    let root_row = IndexRow {
        id: *root().as_bytes(),
        parent_id: None,
        full_hash: root_full,
        own_hash: root_own,
        metadata: Metadata::new(1, 1),
        deleted_at: None,
        deleted_children: Vec::new(),
    };
    let mut records = vec![SnapshotRecord::Entity {
        id: *root().as_bytes(),
        entry: root_entry,
        index: root_row.encode(),
        schema_bytecode_id: None,
    }];
    for (id, entry, row) in children {
        records.push(SnapshotRecord::Entity {
            id: *id.as_bytes(),
            entry: entry.clone(),
            index: row.encode(),
            schema_bytecode_id: None,
        });
    }
    (records, Hash::from(root_full))
}

/// A user-owned entry whose signature verifies against nothing.
fn unsigned_user_entry() -> StorageType {
    StorageType::User {
        rules: EntryRules::OWNED,
        owner: AccountId::from([0xA0; 32]),
        signature_data: Some(SignatureData {
            signature: [0x5A; 64],
            nonce: 1,
            signer: Some(PublicKey::from([9; 32])),
            on_behalf: None,
        }),
    }
}

/// The message of the error an install of `records` ends in, with the context
/// left as it was.
async fn refused(joiner: &Joiner, claimed: Hash, records: Vec<SnapshotRecord>) -> String {
    joiner.holds_stale_state();
    let outcome = joiner.installs(claimed, records).await;
    let message = outcome
        .expect_err("installed a tree that lacks an entity")
        .to_string();
    assert_eq!(joiner.keys(), vec![STALE]);
    assert_eq!(
        joiner.manager.check_sync_in_progress(context()).unwrap(),
        None
    );
    assert!(joiner.stages_nothing());
    message
}

/// The entity ships its real index row, so the root still folds through it, and
/// bytes that are not the row's: leaving it out would hide the hole.
#[tokio::test]
async fn an_entity_shipped_with_bytes_that_are_not_its_hash_fails_the_snapshot() {
    let entity = Id::new([0x81; 32]);
    let (records, claimed) =
        tree_over(&[child(entity, b"real", b"garbage", unsigned_user_entry())]);

    let message = refused(&joiner().await, claimed, records).await;

    assert!(message.contains("own hash"), "{message}");
}

/// The bytes are the row's, and the signature is not: the same hole.
#[tokio::test]
async fn an_entity_whose_signature_fails_fails_the_snapshot() {
    let entity = Id::new([0x82; 32]);
    let (records, claimed) =
        tree_over(&[child(entity, b"forged", b"forged", unsigned_user_entry())]);

    let message = refused(&joiner().await, claimed, records).await;

    assert!(message.contains("signature"), "{message}");
}

/// A leaf held back for its schema is buffered, but its signature does not
/// depend on the schema, so it is checked now.
#[tokio::test]
async fn a_leaf_declined_for_its_schema_with_a_bad_signature_fails_the_snapshot() {
    let leaf = Id::new([0x83; 32]);
    let (mut records, claimed) =
        tree_over(&[child(leaf, b"forged", b"forged", unsigned_user_entry())]);
    *entity(&mut records, leaf).2 = Some([2; 32]);
    let joiner = joiner().await;
    calimero_context::activation::record_activation(&joiner.store, &context(), [1; 32]);

    let message = refused(&joiner, claimed, records).await;

    assert!(message.contains("signature"), "{message}");
    let buffered = AbsorbRepository::new(&joiner.store)
        .enumerate_pending(&context())
        .unwrap();
    assert!(buffered.is_empty());
}

/// A member is checked once every anchor has arrived; one whose anchor never did
/// is left out of the second pass, and out of the tree with it.
#[tokio::test]
async fn a_member_whose_anchor_is_not_in_the_snapshot_fails_the_snapshot() {
    let (member, anchor) = (Id::new([0x71; 32]), Id::new([0x72; 32]));
    let (records, claimed) = tree_over(&[child(
        member,
        b"member",
        b"member",
        StorageType::SharedMember {
            anchor,
            signature_data: None,
        },
    )]);

    let message = refused(&joiner().await, claimed, records).await;

    assert!(message.contains("anchor"), "{message}");
}

/// A `User` entry of `owner`, holding `data`, signed by `relay` on the owner's behalf.
fn written_through_a_relay(
    id: Id,
    data: &[u8],
    relay: &PrivateKey,
    owner: AccountId,
) -> StorageType {
    let signed = |signature| StorageType::User {
        rules: EntryRules::OWNED,
        owner,
        signature_data: Some(SignatureData {
            signature,
            nonce: 1,
            signer: Some(relay.public_key()),
            on_behalf: Some(owner),
        }),
    };
    let mut metadata = Metadata::new(1, 1);
    metadata.storage_type = signed([0; 64]);
    let payload = Action::Add {
        id,
        data: data.to_vec(),
        ancestors: vec![],
        metadata,
    }
    .payload_for_signing();
    signed(relay.sign(&payload).unwrap().to_bytes())
}

impl Joiner {
    /// Put the context in a namespace and certify `key` for an account there,
    /// holding no role: what the rows say of a relay once it has been removed.
    fn knows_the_account_of(&self, key: &PublicKey) -> AccountId {
        let namespace = ContextGroupId::from(NAMESPACE);
        MetaRepository::new(&self.store)
            .save(
                &namespace,
                &sample_meta_with_admin(AccountId::from([0xEE; 32])),
            )
            .unwrap();
        register_context_in_group(&self.store, &namespace, &context()).unwrap();
        enrol_member(&self.store, &namespace, key)
    }

    /// Fold a governance op of the namespace group into the joiner's projection.
    fn folds(&self, op: &GroupOp, id: [u8; 32], parents: &[[u8; 32]]) {
        let signed = SignedNamespaceOp {
            version: 1,
            namespace_id: NAMESPACE.into(),
            parent_op_hashes: Vec::new(),
            signer: PublicKey::from([0xAD; 32]),
            nonce: 0,
            op: NamespaceOp::Group {
                group_id: NAMESPACE.into(),
                key_id: [0; 32].into(),
                encrypted: EncryptedGroupOp {
                    nonce: [0; 12],
                    ciphertext: Vec::new(),
                },
                key_rotation: None,
            },
            signature: [0; 64],
            admitter_endorsement: None,
        };
        let hlc = HybridTimestamp::new(Timestamp::new(
            NTP64(1_700_000_000),
            ID::from(NonZeroU64::new(1).unwrap()),
        ));
        self.manager
            .node_state
            .write_scope_projections()
            .ingest_op(&op_from_namespace_op(&signed, Some(op), id, hlc, parents));
    }

    /// Fold the ops that admitted `relay` as a `RelayTee` and later removed it.
    fn folded_a_relay_seated_then_removed(&self, relay: AccountId) {
        self.folds(
            &GroupOp::MemberAdded {
                member: relay,
                role: GroupMemberRole::RelayTee,
            },
            RELAY_SEATED,
            &[],
        );
        self.folds(
            &GroupOp::MemberRemoved {
                member: relay,
                expected_group_state_hash: [0; 32],
                expected_context_state_hashes: Vec::new(),
            },
            RELAY_REMOVED,
            &[RELAY_SEATED],
        );
    }
}

/// Every peer that was there applied the entry while the relay was seated and
/// still holds it, so a joiner that can see the relay was seated takes it too.
#[tokio::test]
async fn an_entry_written_through_a_relay_removed_since_is_installed() {
    let (owner, relay) = (AccountId::from([0xA1; 32]), PrivateKey::from([0x61; 32]));
    let entry = owned_entry_id(Id::new([0x84; 32]), &owner);
    let joiner = joiner().await;
    let relay_account = joiner.knows_the_account_of(&relay.public_key());
    joiner.folded_a_relay_seated_then_removed(relay_account);
    let through = written_through_a_relay(entry, b"kept", &relay, owner);
    let (records, claimed) = tree_over(&[child(entry, b"kept", b"kept", through)]);

    let (applied, _) = joiner.installs(claimed, records).await.unwrap();

    assert_eq!(applied, 2);
    assert!(joiner.holds(entry));
    assert_eq!(
        served_state_root(&joiner.store, context()).unwrap(),
        claimed
    );
}

/// A key of a member no folded op ever seated as a relay signs for nobody: the
/// entry is the source's lie, and fails the snapshot.
#[tokio::test]
async fn an_entry_written_through_a_key_that_was_never_a_relay_fails_the_snapshot() {
    let (owner, relay) = (AccountId::from([0xA1; 32]), PrivateKey::from([0x62; 32]));
    let entry = owned_entry_id(Id::new([0x85; 32]), &owner);
    let joiner = joiner().await;
    let account = joiner.knows_the_account_of(&relay.public_key());
    let seat = GroupOp::MemberAdded {
        member: account,
        role: GroupMemberRole::Member,
    };
    joiner.folds(&seat, RELAY_SEATED, &[]);
    let through = written_through_a_relay(entry, b"forged", &relay, owner);
    let (records, claimed) = tree_over(&[child(entry, b"forged", b"forged", through)]);

    let message = refused(&joiner, claimed, records).await;

    assert!(message.contains("neither its owner"), "{message}");
}

/// A `User` entry under the root, signed by `writer` at `nonce` and shipped
/// dated `updated_at`, which the signature does not cover.
fn signed_at(writer: &PrivateKey, nonce: u64, updated_at: u64) -> (Id, Vec<u8>, IndexRow) {
    let owner = AccountId::from([0xA2; 32]);
    let id = owned_entry_id(Id::new([0x86; 32]), &owner);
    let signed = |signature| StorageType::User {
        rules: EntryRules::OWNED,
        owner,
        signature_data: Some(SignatureData {
            signature,
            nonce,
            signer: Some(writer.public_key()),
            on_behalf: None,
        }),
    };
    let mut metadata = Metadata::new(1, 1);
    metadata.storage_type = signed([0; 64]);
    let payload = Action::Add {
        id,
        data: b"dated".to_vec(),
        ancestors: vec![],
        metadata,
    }
    .payload_for_signing();
    let (id, entry, mut row) = child(
        id,
        b"dated",
        b"dated",
        signed(writer.sign(&payload).unwrap().to_bytes()),
    );
    row.metadata.updated_at = updated_at.into();
    (id, entry, row)
}

/// `updated_at` is not signed, so the serving peer could re-date the entry.
#[tokio::test]
async fn a_signed_entry_is_installed_under_its_signed_date() {
    let leaf = signed_at(&PrivateKey::from([0x63; 32]), 5, time_now());
    let id = leaf.0;
    let (records, claimed) = tree_over(&[leaf]);
    let joiner = joiner().await;

    joiner.installs(claimed, records).await.unwrap();

    let stored = crate::delta_store::read_entity_index_direct(&joiner.store, context(), id)
        .unwrap()
        .map(|index| index.metadata.updated_at());
    assert_eq!(stored, Some(5));
}

/// Dated by a nonce past the drift tolerance, every later write to the entry
/// would look stale, so the snapshot holding it is refused.
#[tokio::test]
async fn an_entry_signed_ahead_of_the_clock_fails_the_snapshot() {
    let ahead = time_now() + 60_000_000_000;
    let (records, claimed) = tree_over(&[signed_at(&PrivateKey::from([0x64; 32]), ahead, 1)]);

    let message = refused(&joiner().await, claimed, records).await;

    assert!(message.contains("signed time"), "{message}");
}

/// A leaf held back for its schema is buffered, but its signed date does not
/// depend on the schema, so it is checked now.
#[tokio::test]
async fn a_leaf_declined_for_its_schema_signed_ahead_of_the_clock_fails_the_snapshot() {
    let ahead = time_now() + 60_000_000_000;
    let leaf = signed_at(&PrivateKey::from([0x65; 32]), ahead, 1);
    let id = leaf.0;
    let (mut records, claimed) = tree_over(&[leaf]);
    *entity(&mut records, id).2 = Some([2; 32]);
    let joiner = joiner().await;
    calimero_context::activation::record_activation(&joiner.store, &context(), [1; 32]);

    let message = refused(&joiner, claimed, records).await;

    assert!(message.contains("signed time"), "{message}");
}

fn nothing_stored(store: &Store) -> bool {
    store
        .handle()
        .iter::<ContextStateKey>()
        .unwrap()
        .entries()
        .next()
        .is_none()
}

/// A buffered leaf drains to the verdict the page apply reaches, and that
/// includes the bytes being the ones the leaf's own hash commits to.
#[test]
fn a_snapshot_leaf_whose_bytes_do_not_match_its_own_hash_is_not_installed() {
    let (source, _) = source();
    let mut records = shipped(&source);
    let leaf = Id::new([0x21; 32]);
    let (entry, index, _) = entity(&mut records, leaf);
    entry.push(1);
    let target = Store::new(Arc::new(InMemoryDB::owned()));

    let outcome = persist_buffered_snapshot_entity(
        &target,
        &NotFolded,
        context(),
        *leaf.as_bytes(),
        entry,
        index,
        &|_| Ok(CellWriters::Genesis),
    )
    .unwrap();

    assert_eq!(outcome, SnapshotEntityDrainOutcome::Refused);
    assert!(nothing_stored(&target));
}

#[test]
fn a_snapshot_leaf_dated_ahead_of_the_clock_is_not_installed_from_the_buffer() {
    let (source, _) = source();
    let mut records = shipped(&source);
    let leaf = Id::new([0x22; 32]);
    let (entry, index, _) = entity(&mut records, leaf);
    let mut dated = decode(index);
    dated.metadata = Metadata::new(1, u64::MAX / 2);
    *index = dated.encode();
    let target = Store::new(Arc::new(InMemoryDB::owned()));

    let outcome = persist_buffered_snapshot_entity(
        &target,
        &NotFolded,
        context(),
        *leaf.as_bytes(),
        entry,
        index,
        &|_| Ok(CellWriters::Genesis),
    )
    .unwrap();

    assert_eq!(outcome, SnapshotEntityDrainOutcome::Refused);
    assert!(nothing_stored(&target));
}

#[test]
fn a_snapshot_leaf_that_is_what_its_hash_says_drains_into_the_store() {
    let (source, _) = source();
    let mut records = shipped(&source);
    let leaf = Id::new([0x21; 32]);
    let (entry, index, _) = entity(&mut records, leaf);
    let target = Store::new(Arc::new(InMemoryDB::owned()));

    let outcome = persist_buffered_snapshot_entity(
        &target,
        &NotFolded,
        context(),
        *leaf.as_bytes(),
        entry,
        index,
        &|_| Ok(CellWriters::Genesis),
    )
    .unwrap();

    assert_eq!(outcome, SnapshotEntityDrainOutcome::Persisted);
}

#[tokio::test]
async fn a_leaf_whose_hashes_agree_with_each_other_but_not_its_parents_record_does_not_fold() {
    let (source, claimed) = source();
    let leaf = Id::new([0x21; 32]);
    let mut records = shipped(&source);
    let (entry, index, _) = entity(&mut records, leaf);
    *entry = vec![0xEE; 17];
    let mut agreeing = decode(index);
    agreeing.own_hash = sha(entry);
    agreeing.full_hash = sha(&agreeing.own_hash);
    *index = agreeing.encode();
    let joiner = joiner().await;
    joiner.holds_stale_state();

    let outcome = joiner.installs(claimed, records).await;

    assert!(
        outcome.is_err(),
        "installed a leaf its parent does not hold"
    );
    assert_eq!(joiner.keys(), vec![STALE]);
}

#[tokio::test]
async fn a_tree_that_folds_to_another_root_than_the_claim_is_not_installed() {
    let (source, _) = source();
    let joiner = joiner().await;
    joiner.holds_stale_state();

    let outcome = joiner
        .installs(Hash::from([0xAB; 32]), shipped(&source))
        .await;

    assert!(outcome.is_err(), "installed a tree under another root");
    assert_eq!(joiner.keys(), vec![STALE]);
}

#[tokio::test]
async fn entities_that_name_each_other_as_parents_are_not_installed() {
    let (source, claimed) = source();
    let mut records = shipped(&source);
    let (a, b) = (Id::new([0x41; 32]), Id::new([0x42; 32]));
    for (id, parent) in [(a, b), (b, a)] {
        let entry = vec![id.as_bytes()[0]; 8];
        let index = IndexRow {
            id: *id.as_bytes(),
            parent_id: Some(*parent.as_bytes()),
            full_hash: sha(&sha(&entry)),
            own_hash: sha(&entry),
            metadata: Metadata::new(1, 1),
            deleted_at: None,
            deleted_children: Vec::new(),
        };
        records.push(SnapshotRecord::Entity {
            id: *id.as_bytes(),
            entry,
            index: index.encode(),
            schema_bytecode_id: None,
        });
    }
    let joiner = joiner().await;
    joiner.holds_stale_state();

    let outcome = joiner.installs(claimed, records).await;

    assert!(outcome.is_err(), "installed a parent chain that loops");
    assert_eq!(joiner.keys(), vec![STALE]);
}

#[tokio::test]
async fn a_refused_snapshot_leaves_no_sync_marker_behind() {
    let (source, _) = source();
    let joiner = joiner().await;

    let outcome = joiner
        .installs(Hash::from([0xAB; 32]), shipped(&source))
        .await;

    assert!(outcome.is_err());
    assert_eq!(
        joiner.manager.check_sync_in_progress(context()).unwrap(),
        None,
        "a refused snapshot must not make the next attempt a crash recovery"
    );
}

#[tokio::test]
async fn a_public_entity_outside_the_tree_is_dropped_and_a_frozen_one_is_kept() {
    let (source, claimed) = source();
    let (public, frozen) = (Id::new([0x61; 32]), Id::new([0x62; 32]));
    let mut records = shipped(&source);
    records.push(orphan(public, StorageType::Public));
    records.push(orphan(frozen, StorageType::Frozen));
    let joiner = joiner().await;

    joiner.installs(claimed, records).await.unwrap();

    assert!(!joiner.holds(public));
    assert!(joiner.holds(frozen));
    assert_eq!(
        served_state_root(&joiner.store, context()).unwrap(),
        claimed
    );
}

/// An entity whose parent is not in the snapshot.
fn orphan(id: Id, storage_type: StorageType) -> SnapshotRecord {
    let entry = vec![id.as_bytes()[0]; 8];
    let own_hash = sha(&entry);
    let mut metadata = Metadata::new(1, 1);
    metadata.storage_type = storage_type;
    let index = IndexRow {
        id: *id.as_bytes(),
        parent_id: Some([0x60; 32]),
        full_hash: sha(&own_hash),
        own_hash,
        metadata,
        deleted_at: None,
        deleted_children: Vec::new(),
    };
    SnapshotRecord::Entity {
        id: *id.as_bytes(),
        entry,
        index: index.encode(),
        schema_bytecode_id: None,
    }
}

#[tokio::test]
async fn an_installed_snapshot_leaves_nothing_staged() {
    let (source, claimed) = source();
    let joiner = joiner().await;

    joiner.installs(claimed, shipped(&source)).await.unwrap();

    assert!(joiner.stages_nothing());
}

/// A crash part way through staging leaves rows behind; the next install must
/// not count them as part of its own tree.
#[tokio::test]
async fn rows_a_crashed_install_left_staged_are_not_installed() {
    let (source, claimed) = source();
    let joiner = joiner().await;
    let left = Id::new([0x5C; 32]);
    let leftover = [context().as_ref(), &StorageKey::Index(left).to_bytes()[..]].concat();
    joiner
        .store
        .raw_put(Column::SnapshotStage, &leftover, b"left by a crash")
        .unwrap();

    joiner.installs(claimed, shipped(&source)).await.unwrap();

    assert!(!joiner.holds(left));
    assert!(joiner.stages_nothing());
}

/// [`source`] after a process applying one more entry row by row, as repair does,
/// was killed once `survived` writes had landed. Returns how many it attempted.
fn source_killed_mid_apply(survived: usize) -> (Store, Hash, usize) {
    let (store, _) = source();
    let attempted = Rc::new(Cell::new(0_usize));
    let lands = {
        let attempted = Rc::clone(&attempted);
        move || {
            attempted.set(attempted.get() + 1);
            attempted.get() <= survived
        }
    };
    let state_key = |key: &StorageKey| ContextStateKey::new(context(), key.to_bytes());
    let read = {
        let handle = store.handle();
        Rc::new(move |key: &StorageKey| {
            let row = handle.get(&state_key(key)).unwrap();
            row.map(|row| row.value.as_ref().to_vec())
        })
    };
    let write = {
        let (store, lands) = (store.clone(), lands.clone());
        Rc::new(move |key: StorageKey, value: &[u8]| {
            if lands() {
                let value = ContextStateValue::from(Slice::from(value.to_vec()));
                store.handle().put(&state_key(&key), &value).unwrap();
            }
            true
        })
    };
    let remove = {
        let store = store.clone();
        Rc::new(move |key: &StorageKey| {
            if lands() {
                store.handle().delete(&state_key(key)).unwrap();
            }
            true
        })
    };
    let env = RuntimeEnv::new(read, write, remove, CONTEXT, [2; 32], [0xAC; 32]);
    with_runtime_env(env, || {
        let ancestors = [Id::new([2; 32]), root()]
            .map(|parent| ChildInfo::new(parent, [0; 32], Metadata::default()))
            .to_vec();
        let add = Action::Add {
            id: Id::new([0x24; 32]),
            data: vec![0x24; 17],
            ancestors,
            metadata: Metadata::new(2, 2),
        };
        // What the apply returns once its writes stop landing is what a dead
        // process would have seen: only the rows left behind matter.
        let _killed = Interface::<MainStorage>::apply_action(add, &ApplyContext::empty());
    });
    let hash = served_state_root(&store, context()).unwrap();
    (store, hash, attempted.get())
}

/// An honest node killed part way through one apply holds a tree whose hashes
/// no longer agree. A joiner refuses it, and the source can name the entity.
#[tokio::test]
async fn a_source_killed_part_way_through_an_apply_is_refused_and_can_say_where() {
    let (_, _, attempted) = source_killed_mid_apply(usize::MAX);
    let mut refused = Vec::new();
    for survived in 0..=attempted {
        let (source, claimed, _) = source_killed_mid_apply(survived);
        let defect = first_unfolding_entity(&source.handle(), context()).unwrap();
        let outcome = joiner().await.installs(claimed, shipped(&source)).await;
        match (outcome, defect) {
            (Ok(_), None) => {}
            (Err(error), Some((entity, _))) => {
                let named = format!("{:?}", entity.as_bytes());
                assert!(error.to_string().contains(&named), "{error}");
                refused.push(survived);
            }
            (outcome, defect) => panic!("after {survived} writes: {outcome:?}, {defect:?}"),
        }
    }
    assert_eq!((attempted, refused), (6, vec![1, 2, 3, 4]));
}

/// A tree written to completion, deletions included, has nothing to report.
#[test]
fn a_tree_written_to_completion_folds() {
    for store in [source().0, app_source().0] {
        let defect = first_unfolding_entity(&store.handle(), context()).unwrap();
        assert_eq!(defect, None);
    }
}
