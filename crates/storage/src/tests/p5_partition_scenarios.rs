//! Phase **P5** of [#2233](https://github.com/calimero-network/core/issues/2233):
//! cross-node integration tests for the four motivating partition scenarios.
//!
//! Each test simulates 2–3 "nodes" — each backed by its own
//! [`MockedStorage`] scope — and feeds them the same [`CausalDelta`]s in
//! different orders. With the P3 verifier swap and rotation-log write hook
//! in place, the four #2197 scenarios should resolve correctly:
//!
//! - **Update-vs-rotation race** (Bob writes pre-rotation under partition;
//!   rotation lands first on Carol; Bob's write must be accepted).
//! - **Self-removal mid-flight** (writer rotates self out, in-flight updates
//!   causally-before the rotation are accepted, after rejected).
//! - **Concurrent conflicting rotations** (two writers issue rotations
//!   concurrently — deterministic convergence per ADR 0001).
//! - **Long-partition reconciliation** (many rotations + writes on each side,
//!   then merge — both sides converge to the same final state).
//!
//! The fifth listed motivator (bootstrap race) is deferred per ADR 0001 —
//! see "What we explicitly do NOT decide here".
//!
//! These tests run at the storage layer rather than via the WASM/sync
//! harnesses because the production WASM ABI doesn't yet thread
//! `CausalDelta.{id,hlc,parents}` through to `apply_action` — that's a
//! separate follow-up. The storage-layer simulation gives us full control
//! over delivery order and DAG ancestry, which is what the scenarios
//! actually exercise.

use core::num::NonZeroU128;
use std::collections::{HashMap, HashSet};

use ed25519_dalek::SigningKey;

use crate::action::Action;
use crate::address::Id;
use crate::entities::{ChildInfo, Metadata};
use crate::env;
use crate::index::Index;
use crate::interface::{disable_nonce_check_for_testing, ApplyContext, Interface, StorageError};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::rotation_log;
use crate::store::{MockedStorage, StorageAdaptor};
use crate::tests::common::{build_signed_shared_action, pubkey_of};

// =============================================================================
// Harness
// =============================================================================

/// Tracks a DAG of deltas by parent links. Tests build this incrementally as
/// they author deltas; the `happens_before` predicate is then derived via
/// reverse BFS from the second argument.
struct Dag {
    parents: HashMap<[u8; 32], Vec<[u8; 32]>>,
}

impl Dag {
    fn new() -> Self {
        Self {
            parents: HashMap::new(),
        }
    }

    /// Record a delta and its parents.
    fn record(&mut self, delta_id: [u8; 32], parents: Vec<[u8; 32]>) {
        self.parents.insert(delta_id, parents);
    }

    /// `true` iff `ancestor` is in the transitive ancestry of `descendant`.
    /// Same delta returns `false` (not strictly happens-before).
    fn happens_before(&self, ancestor: &[u8; 32], descendant: &[u8; 32]) -> bool {
        if ancestor == descendant {
            return false;
        }
        let mut frontier: Vec<[u8; 32]> = self.parents.get(descendant).cloned().unwrap_or_default();
        let mut seen: HashSet<[u8; 32]> = HashSet::new();
        while let Some(node) = frontier.pop() {
            if !seen.insert(node) {
                continue;
            }
            if node == *ancestor {
                return true;
            }
            if let Some(ps) = self.parents.get(&node) {
                frontier.extend(ps.iter().copied());
            }
        }
        false
    }
}

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

/// Apply `delta` to a node identified by const-generic `SCOPE`. The DAG
/// closure is taken by reference so the caller controls its lifetime; the
/// rotation-log write hook and verifier both consult it.
fn deliver<S: StorageAdaptor>(delta: &Delta, dag: &Dag) -> Result<(), StorageError> {
    let happens_before: &dyn Fn(&[u8; 32], &[u8; 32]) -> bool = &|a, b| dag.happens_before(a, b);
    let ctx = ApplyContext {
        causal_parents: &delta.parents,
        delta_id: Some(delta.id),
        delta_hlc: Some(hlc(delta.hlc_ns)),
        happens_before: Some(happens_before),
    };
    Interface::<S>::apply_action(delta.action.clone(), ctx)
}

fn make_signing_key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

fn setup_root<S: StorageAdaptor>() -> ChildInfo {
    let root_id = Id::root();
    let root_meta = Metadata::default();
    Index::<S>::add_root(ChildInfo::new(root_id, [0; 32], root_meta.clone())).unwrap();
    ChildInfo::new(root_id, [0; 32], root_meta)
}

fn one_sec(n: u64) -> u64 {
    n.saturating_mul(1_000_000_000)
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
/// Without P3 this is the failure mode #2197 calls out: Carol's stored
/// writer set after applying the rotation is {Alice}, so she'd reject
/// Bob's signature against the stored set.
#[test]
fn update_vs_rotation_race_pre_rotation_write_accepted() {
    env::reset_for_testing();
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<500>;
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
    //
    // KNOWN FRAGILITY (PR #2265 review): D2 is correctly skipped here only
    // because the index's `storage_type.writers` is *frozen at bootstrap* —
    // `Index::update_hash_for` updates `own_hash`/`full_hash`/`updated_at`
    // but never touches `storage_type`. So `pre_apply_writers` returned to
    // `maybe_append_rotation_log` is `{Alice, Bob}` (bootstrap), not the
    // post-D1 `{Alice}`. The comparison `pre_apply_writers != action.writers`
    // is thus `{A,B} != {A,B}` → not a rotation. If `update_hash_for` is
    // ever changed to keep `storage_type` in sync, the rotation-detection
    // logic must move to comparing against `writers_at(causal_parents)`
    // instead of stored, or stale value-writes will be falsely logged.
    // See `write_hook_relies_on_stale_stored_writers_for_rotation_detection`
    // in p3_dag_causal.rs for the explicit demonstration. Tracked in #2233 P3.
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
    env::reset_for_testing();
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<510>;
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
    // Per ADR she's no longer a writer at this causal point and must be rejected.
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
        "post-rotation write by removed writer must be rejected; got {:?}",
        post_result
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
    env::reset_for_testing();
    let _nonce_off = disable_nonce_check_for_testing();
    type Carol = MockedStorage<520>;
    type Dave = MockedStorage<521>;
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
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![dave_root.clone()],
        ),
        ..Delta {
            id: d_root_id,
            parents: vec![],
            hlc_ns: one_sec(10),
            action: Action::Compare { id: Id::root() },
        }
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

    let carol_writers = rotation_log::writers_at::<Carol, _>(id, &causal_frontier, &happens_before)
        .unwrap()
        .unwrap();
    let dave_writers = rotation_log::writers_at::<Dave, _>(id, &causal_frontier, &happens_before)
        .unwrap()
        .unwrap();

    assert_eq!(carol_writers, dave_writers, "deterministic convergence");
    assert_eq!(
        carol_writers,
        [bob].into_iter().collect(),
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
///
/// The chains here are small (3 deltas per side) but the structure exercises
/// the same logic at any depth: a long chain of causally-ordered rotations
/// reduces to "the latest in causal order wins" per ADR 0001.
#[test]
fn long_partition_reconciliation_converges() {
    env::reset_for_testing();
    let _nonce_off = disable_nonce_check_for_testing();
    type Left = MockedStorage<530>;
    type Right = MockedStorage<531>;
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
        action: build_signed_shared_action(
            true,
            id,
            b"v0".to_vec(),
            [alice, bob].into_iter().collect(),
            one_sec(10),
            &alice_sk,
            vec![right_root.clone()],
        ),
        ..Delta {
            id: g0,
            parents: vec![],
            hlc_ns: one_sec(10),
            action: Action::Compare { id: Id::root() },
        }
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

    let left_writers = rotation_log::writers_at::<Left, _>(id, &frontier, &hb)
        .unwrap()
        .unwrap();
    let right_writers = rotation_log::writers_at::<Right, _>(id, &frontier, &hb)
        .unwrap()
        .unwrap();

    assert_eq!(
        left_writers, right_writers,
        "both sides converge on the same writer set as-of {{L2, R2}}"
    );

    // Per ADR: between L1 (HLC=20) and R1 (HLC=25), neither happens-before the
    // other (siblings of g0). HLC tiebreak picks R1 — winner is {Bob, Dave}.
    assert_eq!(
        left_writers,
        [bob, dave].into_iter().collect(),
        "R1 (HLC 25) wins HLC tiebreak vs L1 (HLC 20)"
    );
}
