//! Machine-checked algebraic laws for Tree-Fugue.
//!
//! Convergence rests on a handful of algebraic facts, not on how many random
//! scripts happen to be green. This file states each fact as a law and checks
//! it over a *closed* domain, so a shrunken search cannot pass silently: every
//! enumeration asserts its own cardinality.
//!
//! # Kani
//!
//! Every law is written so the same body is a Kani proof harness when the
//! crate is compiled under `cfg(kani)` and an ordinary exhaustive `#[test]`
//! otherwise. Kani is **not installed in this checkout** (`cargo kani` is not a
//! cargo subcommand), and this crate deliberately gains no dependency on it:
//! the `kani::` calls live behind `#[cfg(kani)]` and are therefore not
//! type-checked here. What actually runs today — and in CI — is the
//! `not(kani)` branch of each law, which enumerates a bounded domain
//! exhaustively rather than sampling it.
//!
//! # Which layer each law is checked on
//!
//! * Laws 1 and 2 are properties of the *ordering function* and only exist on
//!   the pure layer ([`calimero_storage::collections::fugue`]); the storage
//!   collection has no comparator and no integration order of its own — it
//!   builds a tree and asks it. They are noted as pure-only in place.
//! * Laws 3 to 6 are properties of the state-based **join**, and the join is
//!   what production actually runs. They are checked twice: once on the pure
//!   node-set join, and once end to end through the real
//!   `Interface::apply_action` path that the sync layer uses. (An earlier
//!   revision of this work checked only `merge_blocks_from`, which production
//!   never calls for a leaf entity — hence the insistence on the apply path.)

// The `subject__scenario` naming convention this crate uses in its tests.
#![allow(non_snake_case)]
// `cfg(kani)` is set by the Kani verifier, which is an external tool and not a
// dependency of this crate; rustc has no way to know the name is expected.
#![allow(unexpected_cfgs)]
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId, Side};
use calimero_storage::collections::{FugueText, Root};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::{Key, MainStorage};

// ---------------------------------------------------------------------------
// The pure model: replicas, states, and the join.
// ---------------------------------------------------------------------------

/// One edit. `Ins` appends when `pos` lands past the end (runs coalesce);
/// a `pos` in the middle splits a run. Mirrors the alphabet the in-crate
/// sweeps use, so a failure here is directly comparable with one there.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// Insert `.1` at `.0 % (len + 1)`.
    Ins(usize, &'static str),
    /// Delete `[start, start + span)`, clamped to the document.
    Del(usize, usize),
}

/// The five-op alphabet: one appending insert, two mid-document inserts, two
/// deletes (single and spanning).
const ALPHABET: [Op; 5] = [
    Op::Ins(usize::MAX, "x"),
    Op::Ins(1, "y"),
    Op::Ins(2, "zz"),
    Op::Del(1, 1),
    Op::Del(0, 3),
];

/// A replica's synced state: its node set. This is exactly what a state-based
/// join sees, and it is causally closed by construction —
/// [`FugueTree::integrate`] only attaches a node once its parent is attached,
/// so every node in the set has its parent in the set.
type State = Vec<FugueNode>;

/// A pure replica: Algorithm 1 plus the same id allocation `FugueText` uses
/// (a replica's counter is the number of nodes it has minted).
#[derive(Clone, Debug)]
struct Replica {
    tree: FugueTree,
    id: u64,
    next: u32,
}

impl Replica {
    /// A replica seeded with an identical, already-synchronised history.
    fn seeded(id: u64) -> Self {
        let mut replica = Self {
            tree: FugueTree::new(),
            id,
            next: 0,
        };
        for node in seed_nodes() {
            replica.tree.integrate(node);
        }
        replica
    }

    fn insert_str(&mut self, pos: usize, s: &str) {
        for (offset, content) in s.chars().enumerate() {
            let id = (self.id, self.next);
            self.next += 1;
            let _ignored = self.tree.insert(pos + offset, content, id).unwrap();
        }
    }

    fn delete_range(&mut self, start: usize, end: usize) {
        let count = end.min(self.tree.len()).saturating_sub(start);
        for _ in 0..count {
            if self.tree.delete(start).is_err() {
                break;
            }
        }
    }

    /// Apply one op, clamping the position the way the storage-layer harness
    /// does so the two run identical scripts.
    fn apply(&mut self, op: &Op) {
        let len = self.tree.values().chars().count();
        match *op {
            Op::Ins(pos, text) => self.insert_str(pos % (len + 1), text),
            Op::Del(start, span) => {
                let start = start % (len + 1);
                self.delete_range(start, start + span);
            }
        }
    }

    fn state(&self) -> State {
        canonical(&self.tree)
    }
}

/// The shared seed document, authored by replica 0 so neither writer's counter
/// space starts used.
fn seed_nodes() -> State {
    let mut seed = Replica {
        tree: FugueTree::new(),
        id: 0,
        next: 0,
    };
    seed.insert_str(0, "seed");
    seed.state()
}

/// A tree's node set in a canonical order, so two states are equal exactly
/// when they describe the same nodes.
fn canonical(tree: &FugueTree) -> State {
    let mut nodes: State = tree.nodes().copied().collect();
    nodes.sort_by_key(|node| node.id);
    nodes
}

/// Replay a state into a fresh tree.
fn tree_of(state: &State) -> FugueTree {
    let mut tree = FugueTree::new();
    for node in state {
        tree.integrate(*node);
    }
    tree
}

/// The state-based join: deliver both node sets into one tree and read back
/// the result.
///
/// This *is* the production join, not a re-implementation of it:
/// [`FugueTree::integrate`] keeps the first definition of an id and lets a
/// tombstone win over a live copy, in either delivery order. Both inputs are
/// causally closed, so nothing is left buffered and the single pass is exact.
fn merge(a: &State, b: &State) -> State {
    let mut tree = tree_of(a);
    for node in b {
        tree.integrate(*node);
    }
    canonical(&tree)
}

/// The document a state reads back as.
fn values_of(state: &State) -> String {
    tree_of(state).values()
}

/// Whether `id` is tombstoned in `state` (absent counts as "not tombstoned").
fn is_tombstoned(state: &State, id: RawId) -> bool {
    state
        .iter()
        .any(|node| node.id == id && node.value.is_none())
}

/// The `index`-th permutation of `items` in the factorial number system.
///
/// A bijection from `0..items.len()!` onto the permutations, so enumerating
/// the index range enumerates every delivery order exactly once.
fn permutation<T: Clone>(items: &[T], mut index: usize) -> Vec<T> {
    let mut pool: Vec<T> = items.to_vec();
    let mut out = Vec::with_capacity(pool.len());
    for size in (1..=pool.len()).rev() {
        let pick = index % size;
        index /= size;
        out.push(pool.remove(pick));
    }
    out
}

/// `n!`
fn factorial(n: usize) -> usize {
    (1..=n).product()
}

/// The three states produced by giving replica `r + 1` the op `script[r]`.
fn states_from(script: [Op; 3]) -> [State; 3] {
    let mut out = Vec::with_capacity(3);
    for (index, op) in script.iter().enumerate() {
        let mut replica = Replica::seeded(u64::try_from(index).unwrap() + 1);
        replica.apply(op);
        out.push(replica.state());
    }
    [out[0].clone(), out[1].clone(), out[2].clone()]
}

/// The `index`-th point of the 3-replica, 1-op-each script space.
fn script_of(index: usize) -> [Op; 3] {
    [
        ALPHABET[index % 5],
        ALPHABET[(index / 5) % 5],
        ALPHABET[(index / 25) % 5],
    ]
}

// ---------------------------------------------------------------------------
// LAW 1 — the sibling comparator is a strict total order.
// ---------------------------------------------------------------------------

/// Fugue orders the children of one `(parent, side)` bucket by node id
/// ascending; that comparator is the whole tie-break of the algorithm, and the
/// traversal is only well defined if it is a strict total order.
///
/// PURE LAYER ONLY: the storage collection has no comparator of its own — it
/// expands its blocks into a [`FugueTree`] and the tree applies this one. The
/// observable half of the law (siblings surface in id order) is checked below
/// through `values()`.
const fn sibling_lt(a: RawId, b: RawId) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1 < b.1)
}

/// Irreflexivity, antisymmetry, transitivity and totality at one point of the
/// domain.
fn check_strict_total_order(a: RawId, b: RawId, c: RawId) {
    assert!(!sibling_lt(a, a), "comparator is not irreflexive at {a:?}");
    assert!(
        !(sibling_lt(a, b) && sibling_lt(b, a)),
        "comparator is not antisymmetric at {a:?}/{b:?}"
    );
    if sibling_lt(a, b) && sibling_lt(b, c) {
        assert!(
            sibling_lt(a, c),
            "comparator is not transitive at {a:?}/{b:?}/{c:?}"
        );
    }
    if a != b {
        assert!(
            sibling_lt(a, b) || sibling_lt(b, a),
            "comparator is not total: {a:?} and {b:?} are incomparable"
        );
    }
}

/// The ids the exhaustive branch ranges over: every combination of three
/// replica ids and three counters. Distinct replicas, equal replicas with
/// distinct counters, and equal ids are all represented, which is every case
/// the lexicographic comparator distinguishes.
const LAW1_IDS: [RawId; 9] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (1, 0),
    (1, 1),
    (1, 2),
    (2, 0),
    (2, 1),
    (2, 2),
];

/// LAW 1: the sibling comparator is a strict total order.
#[cfg_attr(kani, kani::proof)]
#[cfg_attr(kani, kani::unwind(4))]
#[cfg_attr(not(kani), test)]
fn law1__sibling_comparator_is_a_strict_total_order() {
    #[cfg(kani)]
    {
        // Unbounded: symbolic replica ids and counters, no domain restriction.
        let a: RawId = (kani::any(), kani::any());
        let b: RawId = (kani::any(), kani::any());
        let c: RawId = (kani::any(), kani::any());
        check_strict_total_order(a, b, c);
    }

    #[cfg(not(kani))]
    {
        let mut checked = 0_usize;
        for a in LAW1_IDS {
            for b in LAW1_IDS {
                for c in LAW1_IDS {
                    check_strict_total_order(a, b, c);
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, 729, "the comparator sweep must be exhaustive");

        // The observable half: the comparator is what decides sibling order in
        // the document, so a set of same-bucket siblings must read back in
        // ascending id order whatever order it is delivered in.
        let siblings: State = vec![('d', (2, 0)), ('b', (1, 1)), ('a', (1, 0)), ('c', (1, 7))]
            .into_iter()
            .map(|(value, id)| FugueNode {
                id,
                value: Some(value),
                parent: None,
                side: Side::R,
            })
            .collect();

        let mut observed = 0_usize;
        for index in 0..factorial(siblings.len()) {
            let mut tree = FugueTree::new();
            for node in permutation(&siblings, index) {
                tree.integrate(node);
            }
            assert_eq!(
                tree.values(),
                "abcd",
                "siblings did not surface in ascending id order (permutation {index})"
            );
            observed += 1;
        }
        assert_eq!(observed, 24, "the sibling sweep must be exhaustive");
    }
}

// ---------------------------------------------------------------------------
// LAW 2 — `values()` is a pure function of the node set.
// ---------------------------------------------------------------------------

/// The tree of Figure 3 of the paper: `abcdef`, with `a` and `b` both left
/// children of `c`, so the set is not a plain spine and its order genuinely
/// depends on the comparator.
fn figure_3_nodes() -> State {
    vec![
        FugueNode {
            id: (1, 2),
            value: Some('c'),
            parent: None,
            side: Side::R,
        },
        FugueNode {
            id: (1, 0),
            value: Some('a'),
            parent: Some((1, 2)),
            side: Side::L,
        },
        FugueNode {
            id: (1, 1),
            value: Some('b'),
            parent: Some((1, 2)),
            side: Side::L,
        },
        FugueNode {
            id: (1, 3),
            value: Some('d'),
            parent: Some((1, 2)),
            side: Side::R,
        },
        FugueNode {
            id: (1, 4),
            value: Some('e'),
            parent: Some((1, 3)),
            side: Side::R,
        },
        FugueNode {
            id: (1, 5),
            value: Some('f'),
            parent: Some((1, 4)),
            side: Side::R,
        },
    ]
}

/// Deliver `nodes` in the `index`-th order and assert the read-back is
/// `expected`.
fn check_order_independence(nodes: &State, index: usize, expected: &str) {
    let mut tree = FugueTree::new();
    for node in permutation(nodes, index) {
        tree.integrate(node);
    }
    assert_eq!(
        tree.values(),
        expected,
        "integration order {index} changed the document"
    );
}

/// LAW 2: `values()` depends on the node SET, not on the order the nodes were
/// integrated in.
///
/// PURE LAYER ONLY as stated — the storage collection has no integration order
/// to permute. Its counterpart there is delivery-order independence of the
/// apply path, which law 3 checks end to end.
#[cfg_attr(kani, kani::proof)]
#[cfg_attr(kani, kani::unwind(8))]
#[cfg_attr(not(kani), test)]
fn law2__values_is_a_pure_function_of_the_node_set() {
    let nodes = figure_3_nodes();

    #[cfg(kani)]
    {
        let index: usize = kani::any();
        kani::assume(index < 720);
        check_order_independence(&nodes, index, "abcdef");
    }

    #[cfg(not(kani))]
    {
        let mut checked = 0_usize;
        for index in 0..factorial(nodes.len()) {
            check_order_independence(&nodes, index, "abcdef");
            checked += 1;
        }
        assert_eq!(checked, 720, "the permutation sweep must be exhaustive");

        // The same, on a set produced by three concurrently editing replicas
        // rather than by hand: 5 nodes, every delivery order.
        let mut union: State = Vec::new();
        for (index, op) in ALPHABET.iter().take(3).enumerate() {
            let mut replica = Replica::seeded(u64::try_from(index).unwrap() + 1);
            replica.apply(op);
            for node in replica.state() {
                if !union.iter().any(|seen| seen.id == node.id) {
                    union.push(node);
                }
            }
        }
        let expected = values_of(&union);
        let mut concurrent = 0_usize;
        for index in 0..factorial(union.len()) {
            check_order_independence(&union, index, &expected);
            concurrent += 1;
        }
        assert_eq!(
            concurrent,
            factorial(union.len()),
            "the concurrent-set permutation sweep must be exhaustive"
        );
    }
}

// ---------------------------------------------------------------------------
// LAWS 3-6 — the state-based join, on the pure layer.
// ---------------------------------------------------------------------------

/// Laws 3, 4, 5 and 6 at one point of the state space.
fn check_join_laws(a: &State, b: &State, c: &State) {
    // LAW 3 — commutative.
    assert_eq!(merge(a, b), merge(b, a), "merge is not commutative");
    assert_eq!(
        values_of(&merge(a, b)),
        values_of(&merge(b, a)),
        "merge is not commutative as read back"
    );

    // LAW 4 — associative.
    let left = merge(&merge(a, b), c);
    let right = merge(a, &merge(b, c));
    assert_eq!(left, right, "merge is not associative");
    assert_eq!(
        values_of(&left),
        values_of(&right),
        "merge is not associative as read back"
    );

    // LAW 5 — idempotent.
    assert_eq!(&merge(a, a), a, "merge is not idempotent");
    assert_eq!(&merge(b, b), b, "merge is not idempotent");
    assert_eq!(&merge(c, c), c, "merge is not idempotent");

    // LAW 6 — tombstones are monotone: delete-wins never regresses.
    let joined = merge(a, b);
    for state in [a, b] {
        for node in state {
            if node.value.is_none() {
                assert!(
                    is_tombstoned(&joined, node.id),
                    "node {:?} was tombstoned in an input but is live in the join",
                    node.id
                );
            }
        }
    }
    for node in &joined {
        if node.value.is_some() {
            assert!(
                !is_tombstoned(a, node.id) && !is_tombstoned(b, node.id),
                "node {:?} is live in the join but tombstoned in an input",
                node.id
            );
        }
    }
}

/// LAWS 3-6 on the pure node-set join: commutative, associative, idempotent,
/// and tombstone-monotone.
///
/// EXHAUSTIVE over 3 replicas x 1 op each from the 5-op alphabet = 125 states
/// triples. Wider than the in-crate sweeps, which stop at two replicas.
#[cfg_attr(kani, kani::proof)]
#[cfg_attr(kani, kani::unwind(16))]
#[cfg_attr(not(kani), test)]
fn law3456__join_is_commutative_associative_idempotent_and_delete_wins() {
    #[cfg(kani)]
    {
        let index: usize = kani::any();
        kani::assume(index < 125);
        let [a, b, c] = states_from(script_of(index));
        check_join_laws(&a, &b, &c);
    }

    #[cfg(not(kani))]
    {
        let mut checked = 0_usize;
        for index in 0..125_usize {
            let [a, b, c] = states_from(script_of(index));
            check_join_laws(&a, &b, &c);
            checked += 1;
        }
        assert_eq!(checked, 125, "the join-law sweep must be exhaustive");
    }
}

// ---------------------------------------------------------------------------
// Exhaustive bounded convergence, wider than the in-crate sweeps.
// ---------------------------------------------------------------------------

/// EXHAUSTIVE: 3 replicas, 1 op each, every delivery order.
///
/// The in-crate sweep stops at 2 replicas; three is where associativity stops
/// being implied by commutativity, so this is the first bound at which the
/// join's shape is actually load-bearing. `5^3 = 125` scripts x `3! = 6`
/// delivery orders = 750 checks.
#[test]
fn exhaustive__three_replicas_one_op_each_converge_in_every_delivery_order() {
    let mut checked = 0_usize;
    let mut orders = 0_usize;
    for index in 0..125_usize {
        let states = states_from(script_of(index));
        let reference = {
            let joined = merge(&merge(&states[0], &states[1]), &states[2]);
            values_of(&joined)
        };
        for order in 0..factorial(3) {
            let sequence = permutation(&[0_usize, 1, 2], order);
            let mut acc: State = Vec::new();
            for replica in sequence {
                acc = merge(&acc, &states[replica]);
            }
            assert_eq!(
                values_of(&acc),
                reference,
                "script {index} diverged in delivery order {order}"
            );
            orders += 1;
        }
        checked += 1;
    }
    assert_eq!(checked, 125, "the sweep must be exhaustive, not sampled");
    assert_eq!(orders, 750, "every delivery order must be covered");
}

/// EXHAUSTIVE: 2 replicas, 3 ops each — `5^6 = 15_625` scripts, each merged in
/// both orders and checked against the join.
///
/// The in-crate sweep runs `k = 2` ops; this is `k = 3`, i.e. 25x its script
/// space, on the pure layer where it is affordable to enumerate.
#[test]
fn exhaustive__two_replicas_three_ops_each_converge_in_both_orders() {
    let mut checked = 0_usize;
    for index in 0..15_625_usize {
        let pick = |shift: u32| ALPHABET[(index / 5_usize.pow(shift)) % 5];
        let mut a = Replica::seeded(1);
        let mut b = Replica::seeded(2);
        for shift in 0..3 {
            a.apply(&pick(shift));
        }
        for shift in 3..6 {
            b.apply(&pick(shift));
        }
        let (sa, sb) = (a.state(), b.state());
        let ab = merge(&sa, &sb);
        let ba = merge(&sb, &sa);
        assert_eq!(ab, ba, "script {index} diverged as a node set");
        assert_eq!(
            values_of(&ab),
            values_of(&ba),
            "script {index} diverged as text"
        );
        checked += 1;
    }
    assert_eq!(checked, 15_625, "the sweep must be exhaustive, not sampled");
}

// ---------------------------------------------------------------------------
// The same join laws, through the REAL apply path.
// ---------------------------------------------------------------------------

/// An in-memory main-storage backend owned by one replica.
type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

/// Every replica here is the NATIVE DEFAULT context: `collections::ROOT_ID` is
/// a process-global seeded from the first `context_id()` any code in the
/// binary asks for, so installing a different one poisons `Root::new` for the
/// rest of the process.
const CONTEXT_ID: [u8; 32] = [236_u8; 32];

/// Both replicas must derive the SAME collection id, or their actions build
/// two parallel documents that never meet.
const FIELD: &str = "fugue_laws_doc";

fn new_store() -> Store {
    Rc::new(RefCell::new(HashMap::new()))
}

/// A [`RuntimeEnv`] routing all `MainStorage` I/O into `store`, under `device`.
fn env_for(store: &Store, device: [u8; 32]) -> RuntimeEnv {
    let r = Rc::clone(store);
    let reader = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
    let w = Rc::clone(store);
    let writer = Rc::new(move |key: Key, value: &[u8]| {
        w.borrow_mut()
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let rm = Rc::clone(store);
    let remover = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
    let mut account = device;
    account[1] = 0xAC;
    RuntimeEnv::new(reader, writer, remover, CONTEXT_ID, device, account)
}

/// Device id of replica `n`, distinct in the first 8 bytes.
fn device(n: u8) -> [u8; 32] {
    let mut id = [n; 32];
    id[..8].copy_from_slice(&u64::from(n).to_be_bytes());
    id
}

/// Run `f` in `store` under `device`, returning the delta the commit emits.
fn edit(
    store: &Store,
    device: [u8; 32],
    f: impl FnOnce(&mut Root<FugueText<MainStorage>>),
) -> Vec<u8> {
    clear_pending_delta();
    env::with_runtime_env(env_for(store, device), || {
        let mut doc = Root::<FugueText<MainStorage>>::fetch().expect("document root should exist");
        f(&mut doc);
        doc.commit();
        env::take_last_artifact().expect("commit should emit a delta")
    })
}

/// Land `delta` the way the sync path does: decode it into actions and push
/// each through `Interface::apply_action`, skipping the sender's root entry.
fn land(store: &Store, device: [u8; 32], delta: &[u8]) {
    let actions = match borsh::from_slice::<StorageDelta>(delta).expect("delta should decode") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    env::with_runtime_env(env_for(store, device), || {
        for action in actions {
            if action.id().is_root() {
                continue;
            }
            Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                .expect("remote apply_action should succeed");
        }
    });
}

/// The document text as `store` sees it.
fn text_in(store: &Store, device: [u8; 32]) -> String {
    env::with_runtime_env(env_for(store, device), || {
        Root::<FugueText<MainStorage>>::fetch()
            .expect("document root should exist")
            .get_text()
            .expect("get_text should succeed")
    })
}

/// A store holding the shared, already-synchronised seed document.
fn genesis() -> Store {
    let store = new_store();
    clear_pending_delta();
    env::with_runtime_env(env_for(&store, device(1)), || {
        let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
        doc.insert_str_with_replica(0, 0, "seed")
            .expect("seed insert should succeed");
        doc.commit();
        let _ignored = env::take_last_artifact();
    });
    store
}

/// A byte-for-byte copy of `store`, i.e. another replica bootstrapped from it.
fn fork(store: &Store) -> Store {
    Rc::new(RefCell::new(store.borrow().clone()))
}

/// Apply one op to a replica's document, returning the delta it emitted.
fn edit_op(store: &Store, dev: [u8; 32], replica: u64, op: &Op, model: &mut Replica) -> Vec<u8> {
    let len = model.tree.values().chars().count();
    let delta = edit(store, dev, |doc| match *op {
        Op::Ins(pos, text) => {
            doc.insert_str_with_replica(pos % (len + 1), replica, text)
                .expect("insert should succeed");
        }
        Op::Del(start, span) => {
            let start = start % (len + 1);
            doc.delete_range(start, start + span)
                .expect("delete should succeed");
        }
    });
    model.apply(op);
    delta
}

/// LAWS 3-5 through the REAL apply path, with 3 replicas.
///
/// At this layer commutativity, associativity and idempotence of the join are
/// one observable property: the state a replica reaches must depend only on
/// the SET of deltas it has landed, not on the order it landed them in nor on
/// how many times each arrived. That is what the sync layer relies on, and it
/// is checked here through `Interface::apply_action` — the path production
/// actually runs — rather than through `merge_blocks_from`, which no node ever
/// calls for a leaf entity.
///
/// EXHAUSTIVE: `5^3 = 125` scripts (3 replicas, 1 op each) x `3! = 6` delivery
/// orders. Three replicas is new here; the in-crate apply-path sweeps stop at
/// two.
#[test]
fn law345__apply_path_state_depends_only_on_the_delta_set() {
    let mut checked = 0_usize;
    let mut orders = 0_usize;

    for index in 0..125_usize {
        let script = script_of(index);

        // Three replicas fork from one genesis and edit concurrently.
        let base = genesis();
        let stores = [fork(&base), fork(&base), fork(&base)];
        let mut models = [Replica::seeded(1), Replica::seeded(2), Replica::seeded(3)];
        let mut deltas: Vec<Vec<u8>> = Vec::with_capacity(3);
        for replica in 0..3_usize {
            let n = u8::try_from(replica).unwrap() + 1;
            deltas.push(edit_op(
                &stores[replica],
                device(n),
                u64::from(n),
                &script[replica],
                &mut models[replica],
            ));
        }

        // The oracle: the pure join of the three node sets.
        let expected = values_of(&merge(
            &merge(&models[0].state(), &models[1].state()),
            &models[2].state(),
        ));

        // Every delivery order of the three deltas, into a fresh fork of the
        // genesis, with each delta landed TWICE (idempotence).
        for order in 0..factorial(3) {
            let target = fork(&base);
            for replica in permutation(&[0_usize, 1, 2], order) {
                land(&target, device(9), &deltas[replica]);
                land(&target, device(9), &deltas[replica]);
            }
            assert_eq!(
                text_in(&target, device(9)),
                expected,
                "script {index} diverged from the join in delivery order {order} \
                 (ops {script:?})"
            );
            orders += 1;
        }
        checked += 1;
    }

    assert_eq!(checked, 125, "the apply-path sweep must be exhaustive");
    assert_eq!(orders, 750, "every delivery order must be covered");
}

/// LAW 6 through the REAL apply path: a delete never regresses.
///
/// Once a replica has landed a delete, no later delta — including one authored
/// before the delete, by a replica that still believed the character live —
/// may bring the character back. Checked over every op of the alphabet as the
/// concurrent write, and in both landing orders.
#[test]
fn law6__apply_path_tombstones_are_monotone() {
    let mut checked = 0_usize;

    for (index, op) in ALPHABET.iter().enumerate() {
        let base = genesis();
        let store_a = fork(&base);
        let store_b = fork(&base);
        let mut model_a = Replica::seeded(1);
        let mut model_b = Replica::seeded(2);

        // A deletes the first two characters of the seed.
        let delete = edit_op(&store_a, device(1), 1, &Op::Del(0, 2), &mut model_a);
        // B, concurrently and still seeing them live, does something else.
        let concurrent = edit_op(&store_b, device(2), 2, op, &mut model_b);

        let live_after_delete: Vec<RawId> = model_a
            .state()
            .iter()
            .filter(|node| node.value.is_none())
            .map(|node| node.id)
            .collect();
        assert!(
            !live_after_delete.is_empty(),
            "the delete must actually tombstone something"
        );

        for (tag, first, second) in [
            ("delete-first", &delete, &concurrent),
            ("delete-second", &concurrent, &delete),
        ] {
            let target = fork(&base);
            land(&target, device(9), first);
            land(&target, device(9), second);

            let joined = merge(&model_a.state(), &model_b.state());
            for id in &live_after_delete {
                assert!(
                    is_tombstoned(&joined, *id),
                    "{tag}: the join resurrected {id:?}"
                );
            }
            assert_eq!(
                text_in(&target, device(9)),
                values_of(&joined),
                "{tag}: op {index} ({op:?}) disagreed with the delete-wins join"
            );
            checked += 1;
        }
    }

    assert_eq!(
        checked, 10,
        "both landing orders of all 5 ops must be covered"
    );
}
