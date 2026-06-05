//! Phase **P5** of [#2233](https://github.com/calimero-network/core/issues/2233):
//! cross-node integration tests for the four motivating partition scenarios.
//!
//! Migrated from `calimero_storage::tests::p5_partition_scenarios` per #2266
//! step 5 — the storage crate no longer carries DAG-ancestry knowledge, so
//! these scenarios live where the DAG does. The `deliver` helper now
//! mirrors the production sync-layer flow: load the rotation log, resolve
//! `effective_writers` via [`crate::sync::rotation_log_reader::writers_at`],
//! and apply with the resolved set in `ApplyContext`. See ADR 0001.

use core::num::NonZeroU128;
use std::collections::BTreeSet;

use calimero_primitives::identity::PublicKey;
use calimero_storage::action::Action;
use calimero_storage::address::Id;
use calimero_storage::entities::{ChildInfo, Metadata};
use calimero_storage::index::Index;
use calimero_storage::interface::{
    disable_nonce_check_for_testing, ApplyContext, Interface, StorageError,
};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use calimero_storage::rotation_log;
use calimero_storage::store::{MockedStorage, StorageAdaptor};
use calimero_storage::tests::common::{build_signed_shared_action, pubkey_of};
use ed25519_dalek::SigningKey;

use crate::sync::rotation_log_reader;
use crate::sync::test_helpers::Dag;

// =============================================================================
// Harness
// =============================================================================

// The `Dag` topology mirror is shared with P3 via `crate::sync::test_helpers`.
// Signing/action builders (`build_signed_shared_action`, `pubkey_of`) come from
// `calimero_storage::tests::common`.

/// One delta authored on some node; gets delivered to one or more nodes.
struct Delta {
    id: [u8; 32],
    parents: Vec<[u8; 32]>,
    hlc_ns: u64,
    action: Action,
}

fn hlc(ns: u64) -> HybridTimestamp {
    let node_id = ID::from(NonZeroU128::new(1).unwrap());
    HybridTimestamp::new(Timestamp::new(NTP64(ns), node_id))
}

/// Apply `delta` to a node identified by const-generic `SCOPE`. Mirrors the
/// production sync-layer flow from `delta_store::ContextStorageApplier::apply`:
/// resolve `effective_writers` against the rotation log + DAG, build an
/// `ApplyContext`, then `Interface::apply_action`.
fn deliver<S: StorageAdaptor>(delta: &Delta, dag: &Dag) -> Result<(), StorageError> {
    let entity_id = delta.action.id();
    let effective_writers: Option<
        std::collections::BTreeMap<PublicKey, calimero_storage::entities::OpMask>,
    > = match rotation_log::load::<S>(entity_id)? {
        Some(log) => {
            rotation_log_reader::writers_at(&log, &delta.parents, |a, b| dag.happens_before(a, b))
        }
        None => None,
    };
    let ctx = ApplyContext {
        effective_writers,
        delta_id: Some(delta.id),
        delta_hlc: Some(hlc(delta.hlc_ns)),
    };
    Interface::<S>::apply_action(delta.action.clone(), &ctx)
}

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    let (full_hash, _) = Index::<S>::get_hashes_for(root_id).unwrap().unwrap();
    ChildInfo::new(root_id, full_hash, root_meta)
}

fn one_sec(n: u64) -> u64 {
    n.saturating_mul(1_000_000_000)
}

/// Resolve the writer set as-of a causal frontier by loading the log and
/// running the node-side reader. Used by tests that assert convergence on
/// rotated writer sets across nodes.
fn writers_at_frontier<S: StorageAdaptor, F>(
    id: Id,
    frontier: &[[u8; 32]],
    happens_before: F,
) -> Option<std::collections::BTreeMap<PublicKey, calimero_storage::entities::OpMask>>
where
    F: Fn(&[u8; 32], &[u8; 32]) -> bool,
{
    let log = rotation_log::load::<S>(id).unwrap()?;
    rotation_log_reader::writers_at(&log, frontier, happens_before)
}

// =============================================================================
// Scenario 1: Update-vs-rotation race (#2197 motivator 1)
// =============================================================================

/// Bob writes "world" against the writer set he sees ({Alice, Bob}) under
/// a partition. Concurrently, Alice rotates Bob out. Both deltas reach a
/// third peer Carol — first the rotation, then Bob's write. Carol must
/// accept Bob's write because it's causally-before-or-concurrent-with the
/// rotation; from Bob's view he was authoritatively a writer when he
/// authored the action.
///
/// Without the DAG-causal verifier this is the failure mode #2197 calls out:
/// Carol's stored writer set after applying the rotation is {Alice}, so
/// she'd reject Bob's signature against the stored set.
#[test]
fn update_vs_rotation_race_pre_rotation_write_accepted() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<5500>;
    let root = setup_root::<Carol>();

    let alice_sk = make_signing_key(0xA1);
    let bob_sk = make_signing_key(0xB1);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = Id::new([0x50; 32]);

    let mut dag = Dag::new();

    // D_root: Alice bootstraps the entity with writers = {Alice, Bob}.
    let d_root_id = [0xD0; 32];
    let d_root = Delta {
        id: d_root_id,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"hello".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        ),
    };
    dag.record(d_root.id, d_root.parents.clone());
    deliver::<Carol>(&d_root, &dag).expect("bootstrap delivered to Carol");

    // D1: Alice rotates Bob out. Parent = D_root.
    let d1_id = [0xD1; 32];
    let d1 = Delta {
        id: d1_id,
        parents: vec![d_root_id],
        hlc_ns: one_sec(20),
        action: build_signed_shared_action(
            false,
            id,
            b"hello".to_vec(),
            [alice].into_iter().collect(), // Bob removed
            one_sec(20),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(d1.id, d1.parents.clone());

    // D2: Bob writes "world" — concurrently with D1. Parent = D_root (Bob
    // has no causal knowledge of D1).
    let d2_id = [0xD2; 32];
    let d2 = Delta {
        id: d2_id,
        parents: vec![d_root_id],
        hlc_ns: one_sec(21),
        action: build_signed_shared_action(
            false,
            id,
            b"world".to_vec(),
            [alice, bob].into_iter().collect(), // Bob's view of writers
            one_sec(21),
            &bob_sk,
            vec![],
        ),
    };
    dag.record(d2.id, d2.parents.clone());

    // Delivery to Carol: rotation first, then Bob's write.
    deliver::<Carol>(&d1, &dag).expect("rotation delivered");
    deliver::<Carol>(&d2, &dag).expect(
        "Bob's pre-rotation write must be accepted — writers_at(D2.parents=[D_root]) \
         includes Bob, even though stored writers post-D1 is {Alice}",
    );

    // The rotation log on Carol should have entries from D_root and D1; D2
    // was a value-write whose claimed `{Alice, Bob}` matches the bootstrap
    // set, so it doesn't trigger the rotation hook.
    let log = rotation_log::load::<Carol>(id).unwrap().unwrap();
    assert_eq!(log.entries.len(), 2, "log has D_root and D1");
    assert_eq!(log.entries[0].delta_id, d_root_id);
    assert_eq!(log.entries[1].delta_id, d1_id);
}

// =============================================================================
// Scenario 2: Self-removal mid-flight (#2197 motivator 3)
// =============================================================================

/// Alice rotates herself out (writers go from {Alice, Bob} to {Bob}). She
/// has an in-flight update D2 that's *causally before* the rotation D1 in
/// her own view. D2 should be accepted on a peer that sees both. A second
/// in-flight update D3 that's causally *after* D1 (Alice saw the rotation
/// and tried to write anyway) should be rejected.
#[test]
fn self_removal_mid_flight_pre_accepted_post_rejected() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<5510>;
    let root = setup_root::<Carol>();

    let alice_sk = make_signing_key(0xA2);
    let bob_sk = make_signing_key(0xB2);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = Id::new([0x51; 32]);

    let mut dag = Dag::new();

    // D_root: bootstrap with {Alice, Bob}.
    let d_root_id = [0xE0; 32];
    let d_root = Delta {
        id: d_root_id,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root.clone()],
        ),
    };
    dag.record(d_root.id, d_root.parents.clone());
    deliver::<Carol>(&d_root, &dag).unwrap();

    // D2: Alice's in-flight write — happens-before D1 in her local view.
    let d2_id = [0xE2; 32];
    let d2 = Delta {
        id: d2_id,
        parents: vec![d_root_id],
        hlc_ns: one_sec(15),
        action: build_signed_shared_action(
            false,
            id,
            b"alice-pre".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(15),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(d2.id, d2.parents.clone());

    // D1: Alice rotates self out — its parent is D2 (Alice wrote, then rotated).
    let d1_id = [0xE1; 32];
    let d1 = Delta {
        id: d1_id,
        parents: vec![d2_id],
        hlc_ns: one_sec(20),
        action: build_signed_shared_action(
            false,
            id,
            b"alice-pre".to_vec(),
            [bob].into_iter().collect(), // Alice removes self
            one_sec(20),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(d1.id, d1.parents.clone());

    // D3: Alice tries to write AFTER her own rotation — has D1 as parent.
    let d3_id = [0xE3; 32];
    let d3 = Delta {
        id: d3_id,
        parents: vec![d1_id],
        hlc_ns: one_sec(25),
        action: build_signed_shared_action(
            false,
            id,
            b"alice-post".to_vec(),
            [bob].into_iter().collect(),
            one_sec(25),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(d3.id, d3.parents.clone());

    // Delivery: D1 first (rotation), then D2 (pre-rotation), then D3 (post).
    deliver::<Carol>(&d1, &dag).expect("rotation accepted");
    deliver::<Carol>(&d2, &dag).expect(
        "Alice's pre-rotation write accepted — D2 happens-before D1 in DAG, \
         writers_at(D2.parents=[D_root]) includes Alice",
    );
    let post_result = deliver::<Carol>(&d3, &dag);
    assert!(
        matches!(post_result, Err(StorageError::InvalidSignature)),
        "post-rotation write by removed writer must be rejected; got {post_result:?}",
    );
}

// =============================================================================
// Scenario 3: Concurrent conflicting rotations (#2197 motivator 2)
// =============================================================================

/// Two writers (Alice, Bob) issue conflicting rotations concurrently:
/// Alice rotates Bob out; Bob rotates Alice out. Two peers (Carol, Dave)
/// receive the deltas in opposite orders. Per ADR 0001 the deterministic
/// winner is the rotation with the larger HLC. Both peers must converge to
/// the same final writer set.
#[test]
fn concurrent_conflicting_rotations_deterministic_convergence() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<5520>;
    type Dave = MockedStorage<5521>;
    let carol_root = setup_root::<Carol>();
    let dave_root = setup_root::<Dave>();

    let alice_sk = make_signing_key(0xA3);
    let bob_sk = make_signing_key(0xB3);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let id = Id::new([0x52; 32]);

    let mut dag = Dag::new();

    // Bootstrap with {Alice, Bob}.
    let d_root_id = [0xF0; 32];
    let d_root_carol = Delta {
        id: d_root_id,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![carol_root.clone()],
        ),
    };
    let d_root_dave = Delta {
        id: d_root_id,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![dave_root.clone()],
        ),
    };
    dag.record(d_root_carol.id, d_root_carol.parents.clone());
    deliver::<Carol>(&d_root_carol, &dag).unwrap();
    deliver::<Dave>(&d_root_dave, &dag).unwrap();

    // D1: Alice rotates Bob out. HLC = 20.
    let d1_id = [0xF1; 32];
    let d1 = Delta {
        id: d1_id,
        parents: vec![d_root_id],
        hlc_ns: one_sec(20),
        action: build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [alice].into_iter().collect(),
            one_sec(20),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(d1.id, d1.parents.clone());

    // D2: Bob rotates Alice out — concurrent with D1. HLC = 21 (larger).
    let d2_id = [0xF2; 32];
    let d2 = Delta {
        id: d2_id,
        parents: vec![d_root_id],
        hlc_ns: one_sec(21),
        action: build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [bob].into_iter().collect(),
            one_sec(21),
            &bob_sk,
            vec![],
        ),
    };
    dag.record(d2.id, d2.parents.clone());

    // Carol gets D1 then D2; Dave gets D2 then D1.
    deliver::<Carol>(&d1, &dag).expect("D1 by Alice accepted on Carol");
    deliver::<Carol>(&d2, &dag).expect("D2 by Bob accepted on Carol (concurrent with D1)");

    deliver::<Dave>(&d2, &dag).expect("D2 by Bob accepted on Dave");
    deliver::<Dave>(&d1, &dag).expect("D1 by Alice accepted on Dave (concurrent with D2)");

    // Both nodes' rotation logs should now have D_root, D1, D2 (in delivery
    // order, not causal order). The order DIFFERS by node, but writers_at
    // applied to the same causal frontier should produce the same answer.
    let carol_log = rotation_log::load::<Carol>(id).unwrap().unwrap();
    let dave_log = rotation_log::load::<Dave>(id).unwrap().unwrap();
    assert_eq!(carol_log.entries.len(), 3);
    assert_eq!(dave_log.entries.len(), 3);

    // Query the writer set as-of {D1, D2}. Per ADR: HLC tiebreak — D2 (21)
    // beats D1 (20) — winner is {Bob}.
    let causal_frontier = [d1_id, d2_id];
    let happens_before = |a: &[u8; 32], b: &[u8; 32]| dag.happens_before(a, b);

    let carol_writers =
        writers_at_frontier::<Carol, _>(id, &causal_frontier, &happens_before).unwrap();
    let dave_writers =
        writers_at_frontier::<Dave, _>(id, &causal_frontier, &happens_before).unwrap();

    assert_eq!(carol_writers, dave_writers, "deterministic convergence");
    assert_eq!(
        carol_writers,
        [bob]
            .into_iter()
            .map(|k| (k, calimero_storage::entities::OpMask::FULL))
            .collect::<std::collections::BTreeMap<_, _>>(),
        "D2 (HLC 21) wins over D1 (HLC 20)"
    );
}

// =============================================================================
// Scenario 4: Long-partition reconciliation
// =============================================================================

/// Two nodes are partitioned for a "long time" — each accumulates a chain of
/// writes and rotations. After the partition heals, both nodes deliver each
/// other's deltas. Both should agree on the same final answer for
/// `writers_at` queried over the merged causal frontier.
#[test]
fn long_partition_reconciliation_converges() {
    let _nonce_off = disable_nonce_check_for_testing();
    type Left = MockedStorage<5530>;
    type Right = MockedStorage<5531>;
    let left_root = setup_root::<Left>();
    let right_root = setup_root::<Right>();

    let alice_sk = make_signing_key(0xA4);
    let bob_sk = make_signing_key(0xB4);
    let carol_sk = make_signing_key(0xC4);
    let dave_sk = make_signing_key(0xD4);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    let carol = pubkey_of(&carol_sk);
    let dave = pubkey_of(&dave_sk);
    let id = Id::new([0x53; 32]);

    let mut dag = Dag::new();

    // Pre-partition bootstrap: writers = {Alice, Bob}.
    let g0 = [0x10; 32];
    let bootstrap_left = Delta {
        id: g0,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![left_root.clone()],
        ),
    };
    let bootstrap_right = Delta {
        id: g0,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![right_root.clone()],
        ),
    };
    dag.record(g0, vec![]);
    deliver::<Left>(&bootstrap_left, &dag).unwrap();
    deliver::<Right>(&bootstrap_right, &dag).unwrap();

    // Left side: g0 → L1 (Alice → {Alice, Carol}) → L2 (Carol writes "left").
    let l1 = [0x11; 32];
    let l1_delta = Delta {
        id: l1,
        parents: vec![g0],
        hlc_ns: one_sec(20),
        action: build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [alice, carol].into_iter().collect(),
            one_sec(20),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(l1, vec![g0]);
    deliver::<Left>(&l1_delta, &dag).unwrap();

    let l2 = [0x12; 32];
    let l2_delta = Delta {
        id: l2,
        parents: vec![l1],
        hlc_ns: one_sec(30),
        action: build_signed_shared_action(
            false,
            id,
            b"left".to_vec(),
            [alice, carol].into_iter().collect(),
            one_sec(30),
            &carol_sk,
            vec![],
        ),
    };
    dag.record(l2, vec![l1]);
    deliver::<Left>(&l2_delta, &dag).unwrap();

    // Right side: g0 → R1 (Bob → {Bob, Dave}) → R2 (Dave writes "right").
    let r1 = [0x21; 32];
    let r1_delta = Delta {
        id: r1,
        parents: vec![g0],
        hlc_ns: one_sec(25),
        action: build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [bob, dave].into_iter().collect(),
            one_sec(25),
            &bob_sk,
            vec![],
        ),
    };
    dag.record(r1, vec![g0]);
    deliver::<Right>(&r1_delta, &dag).unwrap();

    let r2 = [0x22; 32];
    let r2_delta = Delta {
        id: r2,
        parents: vec![r1],
        hlc_ns: one_sec(35),
        action: build_signed_shared_action(
            false,
            id,
            b"right".to_vec(),
            [bob, dave].into_iter().collect(),
            one_sec(35),
            &dave_sk,
            vec![],
        ),
    };
    dag.record(r2, vec![r1]);
    deliver::<Right>(&r2_delta, &dag).unwrap();

    // Partition heals: each side delivers the other side's chain.
    deliver::<Left>(&r1_delta, &dag).expect("R1 (Bob's rotation) accepted on Left");
    deliver::<Left>(&r2_delta, &dag).expect("R2 (Dave's write) accepted on Left");
    deliver::<Right>(&l1_delta, &dag).expect("L1 (Alice's rotation) accepted on Right");
    deliver::<Right>(&l2_delta, &dag).expect("L2 (Carol's write) accepted on Right");

    // Both sides queried over the full merged frontier {L2, R2} should agree.
    let frontier = [l2, r2];
    let hb = |a: &[u8; 32], b: &[u8; 32]| dag.happens_before(a, b);

    let left_writers = writers_at_frontier::<Left, _>(id, &frontier, &hb).unwrap();
    let right_writers = writers_at_frontier::<Right, _>(id, &frontier, &hb).unwrap();

    assert_eq!(
        left_writers, right_writers,
        "both sides converge on the same writer set as-of {{L2, R2}}"
    );

    // Per ADR: between L1 (HLC=20) and R1 (HLC=25), neither happens-before the
    // other (siblings of g0). HLC tiebreak picks R1 — winner is {Bob, Dave}.
    assert_eq!(
        left_writers,
        [bob, dave]
            .into_iter()
            .map(|k| (k, calimero_storage::entities::OpMask::FULL))
            .collect::<std::collections::BTreeMap<_, _>>(),
        "R1 (HLC 25) wins HLC tiebreak vs L1 (HLC 20)"
    );
}

// =============================================================================
// Scenario 5: writer-set convergence when a hash-neutral rotation is reconciled
// via HashComparison rather than applied as a delta (core#2716 / #2703)
// =============================================================================

/// A writer-set rotation is **hash-neutral**: the writers live in `Metadata`,
/// which is `#[borsh(skip)]` out of the Merkle hash, so rotating does NOT change
/// the wrapper's hash. HashComparison reconciles entity *trees* by hash, so when
/// a node catches up to a peer's branch via HC (not by applying the rotation as
/// a forward/merge delta) it sees the wrapper as hash-equal, skips it, and never
/// learns the rotation — its rotation log stays stale and `resolve_local`
/// (`SharedStorage::writers()`, the local gate) returns the wrong set. HC does
/// not carry rotation logs at all.
///
/// This is the gap the real-partition e2e (`shared-storage-concurrent-rotation-
/// converge`) exposes once #2716's batch-abort brick is fixed: the data
/// converges but `shared_is_writer(C)` disagrees across nodes (the author of the
/// `{A,C}` rotation says C is a writer; the node that only HC-reconciled says it
/// is not).
///
/// This test reproduces that divergence deterministically, then shows the fix's
/// core convergence property: **unioning the peer's rotation-log entries into
/// the local log** (dedup by `delta_id`, which `rotation_log::append` does)
/// makes `resolve_local` converge — independent of the Merkle hash. The
/// node-sync layer performs this union when it reconciles a peer (the
/// production wiring); here we drive the union directly to assert the property.
#[test]
fn writer_set_diverges_when_rotation_reconciled_via_hc_until_log_union() {
    let _nonce_off = disable_nonce_check_for_testing();
    // "Converged" node that applied BOTH concurrent rotations as deltas.
    type Both = MockedStorage<5540>;
    // "HC node" that reconciled the peer's branch via HashComparison and so
    // never applied the peer's (hash-neutral) rotation as a delta.
    type Hc = MockedStorage<5541>;
    let both_root = setup_root::<Both>();
    let hc_root = setup_root::<Hc>();

    let alice_sk = make_signing_key(0xA5);
    let bob_sk = make_signing_key(0xB5);
    let alice = pubkey_of(&alice_sk);
    let bob = pubkey_of(&bob_sk);
    // Carol is the writer node-1 adds; she holds no node here — we only assert
    // on whether C is in the resolved writer set.
    let carol = pubkey_of(&make_signing_key(0xC5));
    let id = Id::new([0x55; 32]);

    let mut dag = Dag::new();

    // Bootstrap {Alice, Bob} on both nodes.
    let g0 = [0x50; 32];
    let mk_bootstrap = |root: ChildInfo| Delta {
        id: g0,
        parents: vec![],
        hlc_ns: one_sec(10),
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![root],
        ),
    };
    dag.record(g0, vec![]);
    deliver::<Both>(&mk_bootstrap(both_root.clone()), &dag).unwrap();
    deliver::<Hc>(&mk_bootstrap(hc_root.clone()), &dag).unwrap();

    // node-1 (Alice) rotates -> {Alice, Carol}. LATER HLC, so it wins the
    // concurrent tiebreak (mirrors the e2e: node-1's window-2 rotation wins).
    let d_add_c = [0x51; 32];
    let rot_add_c = Delta {
        id: d_add_c,
        parents: vec![g0],
        hlc_ns: one_sec(30),
        action: build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [alice, carol].into_iter().collect(),
            one_sec(30),
            &alice_sk,
            vec![],
        ),
    };
    dag.record(d_add_c, vec![g0]);

    // node-2 (Bob) concurrently rotates -> {Alice} (drops itself). EARLIER HLC.
    let d_drop_b = [0x52; 32];
    let rot_drop_b = Delta {
        id: d_drop_b,
        parents: vec![g0],
        hlc_ns: one_sec(20),
        action: build_signed_shared_action(
            false,
            id,
            b"v0".to_vec(),
            [alice].into_iter().collect(),
            one_sec(20),
            &bob_sk,
            vec![],
        ),
    };
    dag.record(d_drop_b, vec![g0]);

    // The "Both" node applies BOTH rotations as deltas (forward/merge path):
    // its log gets both entries.
    deliver::<Both>(&rot_add_c, &dag).unwrap();
    deliver::<Both>(&rot_drop_b, &dag).unwrap();

    // The "HC node" only ever applies node-2's {Alice} rotation as a delta; it
    // reconciled node-1's branch via HashComparison, which (rotation being
    // hash-neutral) never delivered node-1's {Alice, Carol} rotation. So its log
    // is missing that entry.
    deliver::<Hc>(&rot_drop_b, &dag).unwrap();

    // resolve_local (the SharedStorage::writers() local gate) on each node.
    let both_writers =
        rotation_log::resolve_local(&rotation_log::load::<Both>(id).unwrap().unwrap())
            .expect("Both must resolve a writer set");
    let hc_writers_before =
        rotation_log::resolve_local(&rotation_log::load::<Hc>(id).unwrap().unwrap())
            .expect("Hc must resolve a writer set");

    // THE BUG: the two nodes disagree on the resolved writer set — specifically
    // on Carol. "Both" resolves {Alice, Carol} (node-1's later-HLC rotation
    // wins); the HC node, missing that rotation, resolves {Alice}.
    assert_eq!(
        both_writers,
        [alice, carol].into_iter().collect(),
        "the node that applied both rotations resolves node-1's later-HLC {{A,C}}"
    );
    assert!(
        both_writers.contains(&carol) && !hc_writers_before.contains(&carol),
        "repro of the #2703 divergence: HC node lacks Carol because it never \
         received node-1's hash-neutral rotation (HC carries no rotation log); \
         both={both_writers:?} hc={hc_writers_before:?}"
    );

    // THE FIX (core property): union the peer's rotation-log entries into the
    // HC node's log. `rotation_log::append` dedups by delta_id, so this is a
    // safe set-union — exactly what the node-sync reconciliation must do.
    let both_log = rotation_log::load::<Both>(id).unwrap().unwrap();
    let hc_log = rotation_log::load::<Hc>(id).unwrap().unwrap();
    let hc_seen: std::collections::BTreeSet<[u8; 32]> =
        hc_log.entries.iter().map(|e| e.delta_id).collect();
    for entry in &both_log.entries {
        if !hc_seen.contains(&entry.delta_id) {
            rotation_log::append::<Hc>(id, entry.clone()).unwrap();
        }
    }

    // After the union the HC node resolves the SAME writer set — convergence.
    let hc_writers_after =
        rotation_log::resolve_local(&rotation_log::load::<Hc>(id).unwrap().unwrap()).unwrap();
    assert_eq!(
        hc_writers_after, both_writers,
        "after unioning the peer's rotation log, the HC node converges on the \
         same writer set (Carol is now a writer everywhere)"
    );
}
