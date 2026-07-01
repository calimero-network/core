//! Crash-recovery / persistence tests for `DeltaStore` restart behavior.
//!
//! These exercise the real persistence + reload path (`load_persisted_deltas`)
//! over an in-memory store: a `DeltaStore` is built, `ContextDagDelta` rows are
//! written directly to simulate what was on disk at crash time, then a FRESH
//! `DeltaStore` is built over the SAME store and `load_persisted_deltas` is run
//! — exactly the sequence a node performs on restart.

use std::sync::Arc;

use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_store::db::InMemoryDB;
use calimero_store::Store;

use crate::test_support::{context, delta_store_over, GENESIS};

/// A single non-empty action so the reconstructed delta is classified as a
/// regular delta (a genesis-parented, empty-action delta would be inferred as a
/// checkpoint by `load_persisted_deltas`). The action's entity id is the full
/// `delta_id`, so distinct deltas always target distinct entities — deriving it
/// from a single tag byte would collide whenever two delta ids shared a first
/// byte (and an all-zero tag would alias the root/genesis id).
fn one_action(delta_id: [u8; 32]) -> Vec<Action> {
    vec![Action::Add {
        id: Id::new(delta_id),
        data: delta_id[..3].to_vec(),
        ancestors: vec![],
        metadata: Metadata::default(),
    }]
}

/// Write a `ContextDagDelta` row into the store exactly as the persistence path
/// would, with an explicit `applied` flag.
fn persist_row(
    store: &Store,
    delta_id: [u8; 32],
    parents: Vec<[u8; 32]>,
    applied: bool,
    expected_root_hash: [u8; 32],
    // Passed through verbatim. This is a per-call test convention, NOT an
    // enforced invariant: the B1 phase-0 row uses `Some(..)` (the event-carrying
    // pre-persist shape) and the B2 committed rows use `None` (a clean restart
    // harvests no phantom handler replays). An `applied: true` row with
    // `events: Some(..)` is itself a legitimate production state — a committed
    // row whose handler events were not yet cleared, replayed via
    // `pending_handler_events` on reload — so there is no invariant to assert
    // here; these tests simply don't exercise that combination.
    events: Option<Vec<u8>>,
) {
    let mut handle = store.handle();
    let actions = borsh::to_vec(&one_action(delta_id)).expect("serialize actions");
    handle
        .put(
            &calimero_store::key::ContextDagDelta::new(context(), delta_id),
            &calimero_store::types::ContextDagDelta {
                delta_id,
                parents,
                actions,
                // A zero HLC is safe for these tests: `load_persisted_deltas`
                // only infers a checkpoint from `parents == [genesis] && actions
                // empty` — never from the HLC — and every row here carries a
                // non-empty `one_action`, so none is misclassified. A future row
                // with empty actions would need a real HLC to stay a regular delta.
                hlc: HybridTimestamp::default(),
                applied,
                expected_root_hash,
                events,
                author_id: Some(PublicKey::from([0xBB; 32])),
                governance_position_blob: None,
                delta_signature: None,
            },
        )
        .expect("persist ContextDagDelta row");
}

/// B1 — Phase-0 kill before the atomic heads commit.
///
/// `add_deltas_batch` / `add_delta_internal` pre-persist an event-carrying input
/// as a standalone `applied: false` row BEFORE the DAG apply and BEFORE the
/// atomic `dag_heads` commit that would flip it to `applied: true`. If the node
/// is killed in that window, the row is left `applied: false`.
///
/// Correct restart behavior: such a row — whose state effects were never
/// committed (root hash unchanged, heads never advanced) — must NOT be treated
/// as applied on reload. `load_persisted_deltas` now routes `applied: false`
/// rows through the full `add_delta_internal` path, which re-executes the
/// actions and, only on success, atomically flips the row to `applied: true`
/// and advances the heads. The DAG's applied-state and the persisted `applied`
/// flag therefore always agree — the buggy state (DAG marks it applied while the
/// DB row is still `applied: false`, so the delta re-drives on every restart and
/// advertises never-written state) can no longer occur.
///
/// This harness has no WASM runtime, so the re-driven apply cannot complete;
/// the delta stays unapplied in BOTH the DAG and the DB (consistent), rather
/// than being promoted to applied-without-execution as the pre-fix
/// `restore_applied_delta` path did.
#[tokio::test]
async fn phase0_applied_false_row_not_promoted_on_restart() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let delta_id = [0x01; 32];

    // On-disk state at crash time: the standalone applied:false row, no heads
    // commit (nothing advanced the DAG heads past genesis).
    persist_row(
        &store,
        delta_id,
        vec![GENESIS],
        false,
        [0x11; 32],
        Some(vec![1, 2, 3]),
    );

    // Restart: fresh DeltaStore over the same store, then reload from disk.
    // Keep a clone so we can read the persisted row's flag after reload.
    let (delta_store, _tmp, _rx) = delta_store_over(store.clone()).await;
    let _ = delta_store
        .load_persisted_deltas()
        .await
        .expect("reload from persisted rows");

    let dag_applied = delta_store.dag_has_delta_applied(&delta_id).await;
    let db_applied = store
        .handle()
        .get(&calimero_store::key::ContextDagDelta::new(
            context(),
            delta_id,
        ))
        .expect("read persisted row")
        .is_some_and(|row| row.applied);

    // The core regression guard: the pre-fix bug left the DAG marking the delta
    // applied while the DB row stayed `applied: false`. Those two must agree.
    assert_eq!(
        dag_applied, db_applied,
        "DAG applied-state and the persisted `applied` flag must agree; a mismatch \
         is exactly the pre-fix bug (promoted-to-applied without a committed apply)"
    );
    assert!(
        !dag_applied,
        "with no WASM runtime the re-drive cannot complete, so the uncommitted \
         row must not be promoted to applied on restart"
    );
    assert!(
        !delta_store.get_heads().await.contains(&delta_id),
        "an uncommitted delta must not become a DAG head on restart"
    );
}

/// B2 — merge topology and persisted root hash are byte-identical across restart.
///
/// Two concurrent branches (A, B off genesis) plus a merge delta M (parents
/// [A, B]) are persisted as applied, exactly as a committed merge would leave
/// them on disk. A restart (`load_persisted_deltas` on a fresh `DeltaStore`)
/// must reconstruct the identical DAG: M is the single head (the two branches
/// collapse to one — the merge-vs-sequential shape is preserved) and M's stored
/// `expected_root_hash` and parent set round-trip byte-for-byte.
///
/// Note: the WASM-side CRDT recompute of a merge root hash needs a full context
/// + application and is covered by the e2e sync-catchup suites; this pins the
/// persistence/topology determinism that a deterministic root relies on.
#[tokio::test]
async fn merge_topology_and_root_hash_identical_across_restart() {
    let store = Store::new(Arc::new(InMemoryDB::owned()));
    let (a, b, m) = ([0x0A; 32], [0x0B; 32], [0x0C; 32]);
    let (root_a, root_b, root_m) = ([0xAA; 32], [0xBB; 32], [0xCC; 32]);

    // Concurrent branches off genesis, then a committed merge over both.
    persist_row(&store, a, vec![GENESIS], true, root_a, None);
    persist_row(&store, b, vec![GENESIS], true, root_b, None);
    persist_row(&store, m, vec![a, b], true, root_m, None);

    // First restart.
    let (ds1, _tmp1, _rx1) = delta_store_over(store.clone()).await;
    let _ = ds1.load_persisted_deltas().await.expect("reload #1");

    assert!(ds1.dag_has_delta_applied(&a).await);
    assert!(ds1.dag_has_delta_applied(&b).await);
    assert!(ds1.dag_has_delta_applied(&m).await);

    // The merge collapses both branches into a single head, M. `get_heads()`
    // order is DAG-impl-dependent, so sort before the exact-equality compare.
    let mut heads1 = ds1.get_heads().await;
    heads1.sort_unstable();
    assert_eq!(
        heads1,
        vec![m],
        "merge must leave exactly one head after restart"
    );

    // The merge delta's persisted root hash and parents survive byte-identically.
    let m_reloaded = ds1.get_delta(&m).await.expect("merge delta reloaded");
    assert_eq!(
        m_reloaded.expected_root_hash, root_m,
        "merge root hash must round-trip byte-for-byte across restart"
    );
    let mut parents = m_reloaded.parents.clone();
    parents.sort_unstable();
    let mut expected_parents = vec![a, b];
    expected_parents.sort_unstable();
    assert_eq!(
        parents, expected_parents,
        "merge parent set must be preserved"
    );

    // A second, independent restart reproduces the identical head and root hash
    // — the reconstruction is deterministic, not order-dependent.
    let (ds2, _tmp2, _rx2) = delta_store_over(store).await;
    let _ = ds2.load_persisted_deltas().await.expect("reload #2");
    let mut heads2 = ds2.get_heads().await;
    heads2.sort_unstable();
    assert_eq!(heads2, vec![m]);
    assert_eq!(
        ds2.get_delta(&m)
            .await
            .expect("merge reloaded #2")
            .expected_root_hash,
        root_m,
        "root hash must be identical across repeated restarts"
    );
}
