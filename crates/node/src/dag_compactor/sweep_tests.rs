//! What one compaction sweep leaves in the delta column, on RocksDB.
//!
//! Each test seeds a context with [`ROWS`] delta rows (a linear chain whose
//! last delta is the context's persisted head), runs one sweep with the
//! default thresholds, and reads the column back: how many of the context's
//! rows are left, and how many bytes its slice of the column takes.

use std::sync::Arc;

use actix::Actor;
use calimero_context_client::messages::ContextMessage;
use calimero_context_client::{ContextAtomicKey, ContextGuard};
use calimero_dag::{CausalDelta, DeltaKind};
use calimero_node_primitives::DagCompactionConfig;
use calimero_primitives::context::ContextId;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::config::StoreConfig;
use calimero_store::db::Column;
use calimero_store::key::AsKeyParts;
use calimero_store::tx::Transaction;
use calimero_store::{key, types, Store};
use calimero_store_rocksdb::RocksDB;
use calimero_utils_actix::LazyRecipient;
use dashmap::DashMap;
use tokio::sync::RwLock;

use super::DagCompactor;
use crate::delta_store::DeltaStore;
use crate::test_support::{
    context, context_client_over_with_manager, delta_store_over_with_manager, KeepAlive, GENESIS,
};

/// Rows each test seeds: three times the default eligibility threshold.
const ROWS: u32 = 30_000;

/// Opaque bytes carried on each row, so the column's size tracks its rows.
const PAD: usize = 256;

/// Answers `AcquireContextLock` the way `ContextManager` does: one write lock
/// per context, handed out as an owned guard. Nothing on these paths executes.
struct LockOnlyContextManager {
    lock: Arc<RwLock<ContextId>>,
}

impl Actor for LockOnlyContextManager {
    type Context = actix::Context<Self>;
}

impl actix::Handler<ContextMessage> for LockOnlyContextManager {
    type Result = ();

    fn handle(&mut self, msg: ContextMessage, _ctx: &mut Self::Context) -> Self::Result {
        let ContextMessage::AcquireContextLock { outcome, .. } = msg else {
            return;
        };
        let lock = Arc::clone(&self.lock);
        let _handle = actix::spawn(async move {
            let guard = ContextGuard::write(lock.write_owned().await);
            let _ = outcome.send(Some(ContextAtomicKey(guard)));
        });
    }
}

fn manager() -> LazyRecipient<ContextMessage> {
    let recipient = LazyRecipient::<ContextMessage>::new();
    let init = recipient.clone();
    let _addr = LockOnlyContextManager::create(move |ctx| {
        assert!(init.init(ctx), "context manager recipient init");
        LockOnlyContextManager {
            lock: Arc::new(RwLock::new(context())),
        }
    });
    recipient
}

/// Delta `i` of the chain. The counter sits in the last bytes and the rest is
/// mixed from it, so key order is unrelated to chain order, as with real ids.
fn id(i: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut seed = u64::from(i).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    for chunk in out[..28].chunks_mut(8) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        chunk.copy_from_slice(&seed.to_le_bytes()[..chunk.len()]);
    }
    out[28..].copy_from_slice(&i.to_be_bytes());
    out
}

/// Incompressible bytes, so the SST size tracks the live rows.
fn pad(i: u32) -> Vec<u8> {
    let mut seed = u64::from(i) ^ 0x2545_F491_4F6C_DD1D;
    (0..PAD / 8)
        .flat_map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed.to_le_bytes()
        })
        .collect()
}

/// Parent of delta `i`: the one before it, and `first_parent` for the first.
fn parent(i: u32, first_parent: [u8; 32]) -> [u8; 32] {
    if i == 0 {
        first_parent
    } else {
        id(i - 1)
    }
}

fn open_rocksdb() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = camino::Utf8PathBuf::from_path_buf(dir.path().to_path_buf()).expect("utf8 path");
    let store = Store::open::<RocksDB>(&StoreConfig::new(path)).expect("open rocksdb");
    (dir, store)
}

/// Write [`ROWS`] applied rows of a chain rooted at `first_parent`, and the
/// context row naming its last delta as the head.
fn seed_chain(store: &Store, first_parent: [u8; 32]) {
    let cid = context();
    let actions =
        borsh::to_vec(&Vec::<calimero_storage::action::Action>::new()).expect("encode actions");
    for start in (0..ROWS).step_by(5_000) {
        let rows: Vec<_> = (start..(start + 5_000).min(ROWS))
            .map(|i| (key::ContextDagDelta::new(cid, id(i)), i))
            .collect();
        let mut tx = Transaction::default();
        for (row_key, i) in &rows {
            let i = *i;
            let record = types::ContextDagDelta {
                delta_id: id(i),
                parents: vec![parent(i, first_parent)],
                actions: actions.clone(),
                hlc: HybridTimestamp::default(),
                applied: true,
                checkpoint_root_hash: None,
                events: None,
                author_id: None,
                governance_position_blob: Some(pad(i)),
                delta_signature: None,
                delegation: None,
            };
            tx.put(row_key, borsh::to_vec(&record).expect("encode row").into());
        }
        store.apply(&tx).expect("seed rows");
    }
    let meta = types::ContextMeta::new(
        key::ApplicationMeta::new([0x01; 32].into()),
        GENESIS,
        vec![id(ROWS - 1)],
        None,
    );
    store
        .handle()
        .put(&key::ContextMeta::new(cid), &meta)
        .expect("seed context meta");
}

/// The context's rows still in the delta column.
fn rows_on_disk(store: &Store) -> usize {
    let cid = context();
    let handle = store.handle();
    let mut iter = handle.iter::<key::ContextDagDelta>().expect("iter");
    let mut count = 0;
    let mut next = iter
        .seek(key::ContextDagDelta::new(cid, [0; 32]))
        .expect("seek");
    while let Some(key) = next {
        if key.context_id() != cid {
            break;
        }
        count += 1;
        next = iter.next().expect("next");
    }
    count
}

fn has_row(store: &Store, delta: [u8; 32]) -> bool {
    store
        .handle()
        .has(&key::ContextDagDelta::new(context(), delta))
        .expect("has")
}

/// Bytes the context's slice of the delta column takes on disk, flushed first
/// so the figure is SST bytes rather than memtable estimates.
fn slice_bytes(store: &Store) -> u64 {
    store.flush().expect("flush");
    let lo = key::ContextDagDelta::new(context(), [0; 32]);
    let hi = key::ContextDagDelta::new(context(), [u8::MAX; 32]);
    store
        .approximate_size(
            Column::Delta,
            lo.as_key().as_bytes(),
            hi.as_key().as_bytes(),
        )
        .expect("approximate size")
}

/// A `DeltaStore` over `store`, as a running node holds one.
async fn live_store(
    store: &Store,
    manager: &LazyRecipient<ContextMessage>,
) -> (DeltaStore, tempfile::TempDir, KeepAlive) {
    delta_store_over_with_manager(store.clone(), manager.clone()).await
}

/// Register the chain in the in-memory DAG the way the execute path does for
/// a delta it has just persisted.
async fn register_chain(delta_store: &DeltaStore) {
    for i in 0..ROWS {
        let _cascaded = delta_store
            .add_local_applied_delta(CausalDelta {
                id: id(i),
                parents: vec![parent(i, GENESIS)],
                payload: Vec::new(),
                hlc: HybridTimestamp::default(),
                kind: DeltaKind::Regular,
            })
            .await
            .expect("register delta");
    }
}

fn stores_with(
    entries: impl IntoIterator<Item = DeltaStore>,
) -> Arc<DashMap<ContextId, DeltaStore>> {
    let map = DashMap::new();
    for delta_store in entries {
        let _previous = map.insert(context(), delta_store);
    }
    Arc::new(map)
}

/// One sweep with the default thresholds, over `delta_stores` and every
/// context in `store`.
async fn sweep(
    store: &Store,
    manager: &LazyRecipient<ContextMessage>,
    delta_stores: Arc<DashMap<ContextId, DeltaStore>>,
) {
    let (context_client, _tmp, _keep) =
        context_client_over_with_manager(store.clone(), manager.clone()).await;
    let _pruned =
        DagCompactor::compact_all(delta_stores, context_client, DagCompactionConfig::default())
            .await;
}

/// The retain window, allowing the BFS's one-node overshoot.
fn assert_retain_window(rows: usize) {
    let retain = DagCompactionConfig::default().retain_recent_count;
    assert!(
        (retain..=retain + 1).contains(&rows),
        "expected the {retain}-delta retain window on disk, found {rows} of {ROWS} rows"
    );
}

/// A context nothing has touched since the node started has no `DeltaStore`,
/// and its rows are pruned all the same.
#[actix::test]
async fn a_cold_context_is_pruned_to_the_retain_window() {
    let (_dir, store) = open_rocksdb();
    let manager = manager();
    seed_chain(&store, GENESIS);
    let before = (rows_on_disk(&store), slice_bytes(&store));

    sweep(&store, &manager, stores_with([])).await;

    let after = (rows_on_disk(&store), slice_bytes(&store));
    eprintln!(
        "cold context: rows {} -> {}, delta-column bytes {} -> {}",
        before.0, after.0, before.1, after.1
    );
    assert!(has_row(&store, id(ROWS - 1)), "the head is never pruned");
    assert_retain_window(after.0);
}

/// After a restart a compacted context's DAG cannot be rebuilt from its rows,
/// because the oldest retained row's parent is gone, so its in-memory count
/// says nothing about the rows on disk. The disk is pruned by its own count.
#[actix::test]
async fn a_restarted_context_is_pruned_by_its_rows_on_disk() {
    let (_dir, store) = open_rocksdb();
    let manager = manager();
    // The chain's first parent was pruned by an earlier sweep.
    seed_chain(&store, id(u32::MAX));
    let (delta_store, _tmp, _keep) = live_store(&store, &manager).await;
    let _loaded = delta_store
        .load_persisted_deltas()
        .await
        .expect("load persisted deltas");
    let before = rows_on_disk(&store);

    sweep(&store, &manager, stores_with([delta_store])).await;

    let after = rows_on_disk(&store);
    eprintln!("restarted context: rows {before} -> {after}");
    assert!(has_row(&store, id(ROWS - 1)), "the head is never pruned");
    assert_retain_window(after);
}

/// Deleting rows frees nothing until RocksDB compacts the files they sit in,
/// so a sweep that pruned a context compacts its slice of the delta column.
#[actix::test]
async fn a_sweep_gives_the_pruned_rows_space_back() {
    let (_dir, store) = open_rocksdb();
    let manager = manager();
    seed_chain(&store, GENESIS);
    let (delta_store, _tmp, _keep) = live_store(&store, &manager).await;
    register_chain(&delta_store).await;
    let before = (rows_on_disk(&store), slice_bytes(&store));

    sweep(&store, &manager, stores_with([delta_store])).await;

    let after = (rows_on_disk(&store), slice_bytes(&store));
    eprintln!(
        "live context: rows {} -> {}, delta-column bytes {} -> {}",
        before.0, after.0, before.1, after.1
    );
    assert_retain_window(after.0);
    assert!(
        after.1 * 5 < before.1,
        "the slice must shrink with its rows: {} of {} bytes left",
        after.1,
        before.1
    );
}

/// A delta waiting on a missing parent no longer holds its context's history
/// back. It stays, and so does the parent it already has.
#[actix::test]
async fn a_pending_delta_does_not_block_compaction() {
    let (_dir, store) = open_rocksdb();
    let manager = manager();
    seed_chain(&store, GENESIS);
    let (delta_store, _tmp, _keep) = live_store(&store, &manager).await;
    register_chain(&delta_store).await;
    // Pending: one parent is old history it holds, the other never arrives.
    let pending = [0xEE; 32];
    let applied = delta_store
        .add_delta(
            CausalDelta {
                id: pending,
                parents: vec![id(5), [0xDD; 32]],
                payload: Vec::new(),
                hlc: HybridTimestamp::default(),
                kind: DeltaKind::Regular,
            },
            None,
            None,
            None,
            None,
        )
        .await
        .expect("add pending delta");
    assert!(!applied, "the delta must be pending on its missing parent");

    sweep(&store, &manager, stores_with([delta_store.clone()])).await;

    let rows = rows_on_disk(&store);
    eprintln!("context with a pending delta: rows {ROWS} -> {rows}");
    assert!(
        delta_store.has_delta(&pending).await,
        "a pending delta is never pruned"
    );
    assert!(
        has_row(&store, id(5)) && delta_store.has_delta(&id(5)).await,
        "a pending delta's parent stays, on disk and in the DAG"
    );
    // The window plus the pending delta's parent.
    assert!(
        rows <= DagCompactionConfig::default().retain_recent_count + 2,
        "history must be pruned despite the pending delta: {rows} rows left"
    );
}

/// A cold context is pruned only under its execution lock, the one a local
/// execute or an inbound apply holds while it commits a row with the heads, so
/// the window cannot move under the prune.
#[actix::test]
async fn a_cold_prune_waits_for_the_context_lock() {
    let (_dir, store) = open_rocksdb();
    let manager = manager();
    seed_chain(&store, GENESIS);
    let (context_client, _tmp, _keep) =
        context_client_over_with_manager(store.clone(), manager.clone()).await;
    let writer = context_client
        .acquire_lock(&context())
        .await
        .expect("the stand-in manager hands out the lock");

    let sweep = actix::spawn({
        let context_client = context_client.clone();
        async move {
            DagCompactor::compact_all(
                stores_with([]),
                context_client,
                DagCompactionConfig::default(),
            )
            .await
        }
    });
    // Long enough for the sweep to list the context and queue on its lock.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(
        rows_on_disk(&store),
        ROWS as usize,
        "nothing goes while a writer holds the lock"
    );

    drop(writer);
    let pruned = sweep.await.expect("sweep task");
    assert_eq!(pruned, ROWS as usize - rows_on_disk(&store));
    assert_retain_window(rows_on_disk(&store));
}
