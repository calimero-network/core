//! Machine-checked algebraic laws for Tree-Fugue, on the pure layer and through
//! the real `Interface::apply_action` path. Each law's body doubles as a Kani
//! proof harness; CI runs the `not(kani)` branch, which enumerates exhaustively.

#![allow(non_snake_case)]
#![allow(unexpected_cfgs)]
#![allow(clippy::unwrap_used)]

use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId, Side};
use calimero_storage::collections::{FugueText, Root};
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{device, edit, fork, fugue_genesis, fugue_text_in, land, Store};

#[derive(Clone, Copy, Debug)]
enum Op {
    /// Insert `.1` at `.0 % (len + 1)`.
    Ins(usize, &'static str),
    /// Delete `[start, start + span)`, clamped to the document.
    Del(usize, usize),
}

const ALPHABET: [Op; 5] = [
    Op::Ins(usize::MAX, "x"),
    Op::Ins(1, "y"),
    Op::Ins(2, "zz"),
    Op::Del(1, 1),
    Op::Del(0, 3),
];

/// A replica's node set; causally closed, since a node integrates only after its parent.
type State = Vec<FugueNode>;

/// A pure replica minting ids exactly as `FugueText` does: counter = nodes minted.
#[derive(Clone, Debug)]
struct Replica {
    tree: FugueTree,
    id: u64,
    next: u32,
}

impl Replica {
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

    /// Clamps positions exactly as `edit_op` does, so both layers run identical scripts.
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

/// Authored by replica 0, so neither writer's counter space starts used.
fn seed_nodes() -> State {
    let mut seed = Replica {
        tree: FugueTree::new(),
        id: 0,
        next: 0,
    };
    seed.insert_str(0, "seed");
    seed.state()
}

/// Sorted, so two states compare equal exactly when they hold the same nodes.
fn canonical(tree: &FugueTree) -> State {
    let mut nodes: State = tree.nodes().copied().collect();
    nodes.sort_by_key(|node| node.id);
    nodes
}

fn tree_of(state: &State) -> FugueTree {
    let mut tree = FugueTree::new();
    for node in state {
        tree.integrate(*node);
    }
    tree
}

/// The production join: an id's first definition is kept, and a tombstone wins.
fn merge(a: &State, b: &State) -> State {
    let mut tree = tree_of(a);
    for node in b {
        tree.integrate(*node);
    }
    canonical(&tree)
}

fn values_of(state: &State) -> String {
    tree_of(state).values()
}

/// Absent counts as not tombstoned.
fn is_tombstoned(state: &State, id: RawId) -> bool {
    state
        .iter()
        .any(|node| node.id == id && node.value.is_none())
}

/// Factorial number system: a bijection from `0..items.len()!`.
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

fn factorial(n: usize) -> usize {
    (1..=n).product()
}

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

/// Mirrors the order `FugueTree` gives same-bucket siblings: node id ascending.
const fn sibling_lt(a: RawId, b: RawId) -> bool {
    a.0 < b.0 || (a.0 == b.0 && a.1 < b.1)
}

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

/// Three replica ids x three counters: every case the comparator distinguishes.
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

#[cfg_attr(kani, kani::proof)]
#[cfg_attr(kani, kani::unwind(4))]
#[cfg_attr(not(kani), test)]
fn law1__sibling_comparator_is_a_strict_total_order() {
    #[cfg(kani)]
    {
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

/// Figure 3: not a plain spine, so read-back order really depends on the comparator.
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

/// Pure layer only: the collection has no integration order to permute.
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

/// Laws 3, 4, 5 and 6 at one point of the state space.
fn check_join_laws(a: &State, b: &State, c: &State) {
    assert_eq!(merge(a, b), merge(b, a), "merge is not commutative");
    assert_eq!(
        values_of(&merge(a, b)),
        values_of(&merge(b, a)),
        "merge is not commutative as read back"
    );

    let left = merge(&merge(a, b), c);
    let right = merge(a, &merge(b, c));
    assert_eq!(left, right, "merge is not associative");
    assert_eq!(
        values_of(&left),
        values_of(&right),
        "merge is not associative as read back"
    );

    assert_eq!(&merge(a, a), a, "merge is not idempotent");
    assert_eq!(&merge(b, b), b, "merge is not idempotent");
    assert_eq!(&merge(c, c), c, "merge is not idempotent");

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

/// Three replicas: the first bound where associativity is not implied by commutativity.
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

/// Every replica must use this field name, or they build documents that never meet.
const FIELD: &str = "fugue_laws_doc";

fn genesis() -> Store {
    fugue_genesis(FIELD, "seed")
}

/// Applies `op` to the document and to its model, returning the emitted delta.
fn edit_op(store: &Store, dev: [u8; 32], replica: u64, op: &Op, model: &mut Replica) -> Vec<u8> {
    let len = model.tree.values().chars().count();
    let delta = edit(
        store,
        dev,
        |doc: &mut Root<FugueText<MainStorage>>| match *op {
            Op::Ins(pos, text) => {
                doc.insert_str_with_replica(pos % (len + 1), replica, text)
                    .expect("insert should succeed");
            }
            Op::Del(start, span) => {
                let start = start % (len + 1);
                doc.delete_range(start, start + span)
                    .expect("delete should succeed");
            }
        },
    );
    model.apply(op);
    delta
}

/// The pure join is the oracle: convergence alone passes for a lossy merge too.
#[test]
fn law345__apply_path_state_depends_only_on_the_delta_set() {
    let mut checked = 0_usize;
    let mut orders = 0_usize;

    for index in 0..125_usize {
        let script = script_of(index);

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

        let expected = values_of(&merge(
            &merge(&models[0].state(), &models[1].state()),
            &models[2].state(),
        ));

        // Each delta lands twice: idempotence.
        for order in 0..factorial(3) {
            let target = fork(&base);
            for replica in permutation(&[0_usize, 1, 2], order) {
                land(&target, device(9), &deltas[replica]);
                land(&target, device(9), &deltas[replica]);
            }
            assert_eq!(
                fugue_text_in(&target, device(9)),
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

/// A concurrent delta, authored while the character was live, must not resurrect it.
#[test]
fn law6__apply_path_tombstones_are_monotone() {
    let mut checked = 0_usize;

    for (index, op) in ALPHABET.iter().enumerate() {
        let base = genesis();
        let store_a = fork(&base);
        let store_b = fork(&base);
        let mut model_a = Replica::seeded(1);
        let mut model_b = Replica::seeded(2);

        let delete = edit_op(&store_a, device(1), 1, &Op::Del(0, 2), &mut model_a);
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
                fugue_text_in(&target, device(9)),
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
