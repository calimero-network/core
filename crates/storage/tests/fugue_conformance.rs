//! Conformance to the EXTERNAL specification Tree-Fugue is proven against.
//!
//! The crate's own sweeps check Fugue against a model oracle that is the same
//! algorithm, so the two agree by construction. This file checks it against
//! statements made by somebody else: Weidner, Gentle and Kleppmann, *The Art
//! of the Fugue* (arXiv 2305.00583), and the specification its Theorem 1
//! proves Tree-Fugue satisfies.
//!
//! Attiya et al.'s strong list specification: there is one total order `<` on
//! all elements such that **(a)** a replica's `values()` returns, in `<`,
//! exactly the elements it has received an `insert` and not a `delete` for,
//! and **(b)** if `values()` yields `[a_0, …, a_{n-1}]` just before
//! `insert(i, x)`, the new element `e` satisfies
//! `a_0, …, a_{i-1} < e < a_i, …, a_{n-1}`. Both are tested directly rather
//! than weakened into "the replicas agree", against a `<` witnessed
//! explicitly: every element gets a globally unique character, and the order
//! is read off a shadow tree of every node the execution created, with nothing
//! tombstoned.
//!
//! Non-interleaving (paper Table I, Figure 2): concurrent runs typed at the
//! same position must stay contiguous. The three anomaly columns are not
//! equivalent, so forward, backward single-replica and backward multi-replica
//! (one session whose ids span replicas, as when a user changes device) are
//! separate cases here. This is NOT the paper's *maximal* non-interleaving
//! (Definition 4), which only FugueMax satisfies: plain Fugue loses one
//! backward adjacency when two concurrent inserts share a left origin but
//! differ on the right, pinned by
//! [`figure_7__right_siblings_order_by_id_not_by_right_origin`]. No passage is
//! split either way. The backward scenario also runs against
//! [`ReplicatedGrowableArray`], asserting that RGA DOES interleave, which is
//! what keeps the Fugue cases from passing vacuously.
//!
//! Every property is checked on the pure algorithm AND through the real apply
//! path (`Interface::apply_action`). Where the storage layer needs element
//! identity it borrows it from a pure twin driven by the identical script, and
//! the twin's agreement with the document is asserted at every step, so the
//! identity is not assumed.

// The `subject__scenario` naming convention this crate uses in its tests.
#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId, Side};
use calimero_storage::collections::{FugueText, ReplicatedGrowableArray, Root};
use calimero_storage::delta::clear_pending_delta;
use calimero_storage::env;
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, NTP64};
use calimero_storage::store::MainStorage;

mod fugue_harness;

use fugue_harness::{
    device, edit, env_for, fork, fugue_genesis, fugue_text_in, land, new_store, read_with, Store,
};

// ---------------------------------------------------------------------------
// The pure twin: one replica of the raw algorithm, with element identity.
// ---------------------------------------------------------------------------

/// A pure replica, using the same id allocation `FugueText` does (a replica's
/// counter is the number of nodes it has minted).
#[derive(Clone, Debug)]
struct Twin {
    tree: FugueTree,
    id: u64,
    next: u32,
}

impl Twin {
    fn new(id: u64) -> Self {
        Self {
            tree: FugueTree::new(),
            id,
            next: 0,
        }
    }

    /// Insert one character, returning the node that must be broadcast.
    fn insert(&mut self, pos: usize, value: char) -> FugueNode {
        let node = self.tree.insert(pos, value, (self.id, self.next)).unwrap();
        self.next += 1;
        node
    }

    /// Tombstone the character at `pos`, returning its id.
    fn delete(&mut self, pos: usize) -> RawId {
        self.tree.delete(pos).unwrap()
    }

    fn text(&self) -> String {
        self.tree.values()
    }

    fn len(&self) -> usize {
        self.tree.len()
    }

    /// Deliver every node `other` holds: the state-based join.
    fn receive_from(&mut self, other: &Self) {
        for node in other.tree.nodes().copied().collect::<Vec<_>>() {
            self.tree.integrate(node);
        }
    }

    /// The elements this replica has received, live or tombstoned.
    fn received(&self) -> Vec<RawId> {
        self.tree.nodes().map(|node| node.id).collect()
    }

    /// The elements this replica has received a delete for.
    fn deleted(&self) -> Vec<RawId> {
        self.tree
            .nodes()
            .filter(|node| node.value.is_none())
            .map(|node| node.id)
            .collect()
    }
}

/// The witness for the strong list specification's total order `<`.
///
/// Every element of the execution gets a globally unique character, so the
/// order can be read off as a string: a shadow tree is built from every node
/// the execution ever created, with nothing tombstoned, and its `values()` is
/// `<` written out. This is a *witness*, not a re-derivation: the order comes
/// from the same [`FugueTree`] traversal the replicas use.
#[derive(Debug, Default)]
struct Elements {
    /// Every node ever created, in its live form.
    all: Vec<FugueNode>,
}

impl Elements {
    fn record(&mut self, node: FugueNode) {
        assert!(
            !self.all.iter().any(|seen| seen.id == node.id),
            "element ids must be unique: {:?} minted twice",
            node.id
        );
        self.all.push(node);
    }

    /// The total order `<`, as the sequence of element characters.
    fn order(&self) -> Vec<char> {
        let mut shadow = FugueTree::new();
        for node in &self.all {
            shadow.integrate(*node);
        }
        shadow.values().chars().collect()
    }

    fn char_of(&self, id: RawId) -> char {
        self.all
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| node.value)
            .expect("every recorded element has a character")
    }

    /// Position of `value` in `<`.
    fn position(order: &[char], value: char) -> usize {
        order
            .iter()
            .position(|c| *c == value)
            .expect("every element appears in the total order")
    }
}

/// One `insert(i, x)` call, kept for the property (b) check.
#[derive(Debug)]
struct InsertCall {
    /// The replica's `values()` immediately before the call.
    before: Vec<char>,
    /// The index the caller asked for.
    index: usize,
    /// The character of the element the call created.
    element: char,
}

/// PROPERTY (b): `a_0, …, a_{i-1} < e < a_i, …, a_{n-1}` in the one total
/// order `<`, for every insert the execution performed.
fn assert_property_b(order: &[char], calls: &[InsertCall]) {
    for call in calls {
        let inserted = Elements::position(order, call.element);
        for (offset, neighbour) in call.before.iter().enumerate() {
            let position = Elements::position(order, *neighbour);
            if offset < call.index {
                assert!(
                    position < inserted,
                    "strong list spec (b): {:?} was left of the insertion point but is \
                     not < the inserted element {:?} (call {call:?})",
                    neighbour,
                    call.element
                );
            } else {
                assert!(
                    inserted < position,
                    "strong list spec (b): {:?} was right of the insertion point but is \
                     not > the inserted element {:?} (call {call:?})",
                    neighbour,
                    call.element
                );
            }
        }
    }
}

/// PROPERTY (a): a replica's `values()` is the `<`-ordered restriction of the
/// global element set to what it has received and not deleted.
fn expected_values(order: &[char], elements: &Elements, twin: &Twin) -> String {
    let received: Vec<char> = twin
        .received()
        .into_iter()
        .map(|id| elements.char_of(id))
        .collect();
    let deleted: Vec<char> = twin
        .deleted()
        .into_iter()
        .map(|id| elements.char_of(id))
        .collect();
    order
        .iter()
        .filter(|c| received.contains(c) && !deleted.contains(c))
        .collect()
}

// ---------------------------------------------------------------------------
// The enumerated executions.
// ---------------------------------------------------------------------------

/// One step of an execution: which replica acts, and what it does.
#[derive(Clone, Copy, Debug)]
enum Act {
    /// Insert a fresh element at the front.
    InsFront,
    /// Insert a fresh element in the middle.
    InsMid,
    /// Insert a fresh element at the end.
    InsEnd,
    /// Delete the first element (no-op on an empty document).
    DelFirst,
}

/// The four-act alphabet: three insertion positions (front, middle, end: the
/// three cases of Fugue's `leftOrigin`/`rightOrigin` rule) and a delete.
const ACTS: [Act; 4] = [Act::InsFront, Act::InsMid, Act::InsEnd, Act::DelFirst];

/// Characters handed out as element identities. Unique per element, which is
/// what makes the total order readable as a string.
const POOL: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The index-th point of an execution space of `steps` steps over `replicas`
/// replicas, as `(replica, act)` pairs.
fn script_of(index: usize, steps: usize, replicas: usize) -> Vec<(usize, Act)> {
    let radix = replicas * ACTS.len();
    (0..steps)
        .map(|step| {
            let digit = (index / radix.pow(u32::try_from(step).unwrap())) % radix;
            (digit % replicas, ACTS[digit / replicas])
        })
        .collect()
}

/// The position an act targets in a document of length `len`.
const fn position_for(act: Act, len: usize) -> usize {
    match act {
        Act::InsFront => 0,
        Act::InsMid => len / 2,
        Act::InsEnd => len,
        Act::DelFirst => 0,
    }
}

/// STRONG LIST SPECIFICATION on the pure algorithm.
///
/// EXHAUSTIVE: 2 replicas x 4 acts = 8 choices per step, 4 steps =
/// `8^4 = 4096` executions. Each execution checks property (a) after every
/// step and after each of two syncs, and property (b) for every insert it
/// performed.
#[test]
fn strong_list_spec__holds_on_the_pure_algorithm() {
    const STEPS: usize = 4;
    const REPLICAS: usize = 2;
    let space = (REPLICAS * ACTS.len()).pow(u32::try_from(STEPS).unwrap());

    let mut executions = 0_usize;
    let mut property_a_checks = 0_usize;

    for index in 0..space {
        let mut elements = Elements::default();
        let mut calls: Vec<InsertCall> = Vec::new();
        let mut twins: Vec<Twin> = (0..REPLICAS)
            .map(|r| Twin::new(u64::try_from(r).unwrap() + 1))
            .collect();
        let mut next_char = 0_usize;

        // A shared, already-synchronised seed: replica 0 types one element and
        // everybody receives it.
        {
            let node = twins[0].insert(0, char::from(POOL[next_char]));
            next_char += 1;
            elements.record(node);
            let seed = twins[0].clone();
            for twin in twins.iter_mut().skip(1) {
                twin.receive_from(&seed);
            }
        }

        let check_a = |elements: &Elements, twins: &[Twin], counter: &mut usize| {
            let order = elements.order();
            for twin in twins {
                assert_eq!(
                    twin.text(),
                    expected_values(&order, elements, twin),
                    "strong list spec (a): a replica's values() is not the <-ordered \
                     restriction of the element set to what it received (execution {index})"
                );
                *counter += 1;
            }
        };

        for (replica, act) in script_of(index, STEPS, REPLICAS) {
            let len = twins[replica].len();
            let position = position_for(act, len);
            match act {
                Act::DelFirst => {
                    if len > 0 {
                        let _ignored = twins[replica].delete(position);
                    }
                }
                _ => {
                    let value = char::from(POOL[next_char]);
                    next_char += 1;
                    let before: Vec<char> = twins[replica].text().chars().collect();
                    let node = twins[replica].insert(position, value);
                    elements.record(node);
                    calls.push(InsertCall {
                        before,
                        index: position,
                        element: value,
                    });
                }
            }
            check_a(&elements, &twins, &mut property_a_checks);
        }

        // Sync one way, then the other, checking (a) at each partial state.
        let snapshot = twins[1].clone();
        twins[0].receive_from(&snapshot);
        check_a(&elements, &twins, &mut property_a_checks);
        let snapshot = twins[0].clone();
        twins[1].receive_from(&snapshot);
        check_a(&elements, &twins, &mut property_a_checks);

        assert_property_b(&elements.order(), &calls);
        executions += 1;
    }

    assert_eq!(executions, 4096, "the execution sweep must be exhaustive");
    assert_eq!(
        property_a_checks,
        4096 * REPLICAS * (STEPS + 2),
        "property (a) must be checked at every observable point"
    );
}

/// STRONG LIST SPECIFICATION through the storage collection's REAL apply path.
///
/// The document runs the identical script as a pure twin and the two are
/// asserted equal at every step, so element identity, which `FugueText` does
/// not expose, is borrowed from the twin without being assumed.
///
/// EXHAUSTIVE: 2 replicas x 4 acts = 8 choices per step, 3 steps =
/// `8^3 = 512` executions through `Interface::apply_action`.
#[test]
fn strong_list_spec__holds_through_the_apply_path() {
    const STEPS: usize = 3;
    const REPLICAS: usize = 2;
    const FIELD: &str = "conformance_strong_list_doc";
    let space = (REPLICAS * ACTS.len()).pow(u32::try_from(STEPS).unwrap());

    let mut executions = 0_usize;
    let mut property_a_checks = 0_usize;

    for index in 0..space {
        let mut elements = Elements::default();
        let mut calls: Vec<InsertCall> = Vec::new();
        let mut next_char = 0_usize;

        // The shared seed: one element, authored by replica 0, present on both.
        let seed_char = char::from(POOL[next_char]);
        next_char += 1;
        let base = fugue_genesis(FIELD, &seed_char.to_string());
        let stores = [fork(&base), fork(&base)];
        let mut twins: Vec<Twin> = (0..REPLICAS)
            .map(|r| Twin::new(u64::try_from(r).unwrap() + 1))
            .collect();
        {
            let mut seed = Twin::new(0);
            let node = seed.insert(0, seed_char);
            elements.record(node);
            for twin in twins.iter_mut() {
                twin.receive_from(&seed);
            }
        }
        let mut deltas: Vec<Vec<Vec<u8>>> = vec![Vec::new(); REPLICAS];

        for (replica, act) in script_of(index, STEPS, REPLICAS) {
            let dev = device(u8::try_from(replica).unwrap() + 1);
            let replica_id = u64::try_from(replica).unwrap() + 1;
            let before: Vec<char> = fugue_text_in(&stores[replica], dev).chars().collect();
            let len = before.len();
            let position = position_for(act, len);

            match act {
                Act::DelFirst => {
                    if len > 0 {
                        let delta = edit::<FugueText<MainStorage>>(&stores[replica], dev, |doc| {
                            doc.delete_range(position, position + 1)
                                .expect("delete should succeed");
                        });
                        deltas[replica].push(delta);
                        let _ignored = twins[replica].delete(position);
                    }
                }
                _ => {
                    let value = char::from(POOL[next_char]);
                    next_char += 1;
                    let delta = edit::<FugueText<MainStorage>>(&stores[replica], dev, |doc| {
                        doc.insert_str_with_replica(position, replica_id, &value.to_string())
                            .expect("insert should succeed");
                    });
                    deltas[replica].push(delta);
                    let node = twins[replica].insert(position, value);
                    elements.record(node);
                    calls.push(InsertCall {
                        before,
                        index: position,
                        element: value,
                    });
                }
            }

            let order = elements.order();
            for r in 0..REPLICAS {
                let dev = device(u8::try_from(r).unwrap() + 1);
                let text = fugue_text_in(&stores[r], dev);
                assert_eq!(
                    text,
                    twins[r].text(),
                    "the document and its pure twin disagree (execution {index})"
                );
                assert_eq!(
                    text,
                    expected_values(&order, &elements, &twins[r]),
                    "strong list spec (a): the document's get_text is not the <-ordered \
                     restriction of the element set to what it received (execution {index})"
                );
                property_a_checks += 1;
            }
        }

        // Sync both ways through the apply path, re-checking (a) each time.
        for (from, to) in [(1_usize, 0_usize), (0, 1)] {
            let dev = device(u8::try_from(to).unwrap() + 1);
            for delta in &deltas[from] {
                land(&stores[to], dev, delta);
            }
            let snapshot = twins[from].clone();
            twins[to].receive_from(&snapshot);

            let order = elements.order();
            for r in 0..REPLICAS {
                let dev = device(u8::try_from(r).unwrap() + 1);
                let text = fugue_text_in(&stores[r], dev);
                assert_eq!(
                    text,
                    twins[r].text(),
                    "the document and its pure twin disagree after sync (execution {index})"
                );
                assert_eq!(
                    text,
                    expected_values(&order, &elements, &twins[r]),
                    "strong list spec (a) after sync (execution {index})"
                );
                property_a_checks += 1;
            }
        }

        assert_property_b(&elements.order(), &calls);
        executions += 1;
    }

    assert_eq!(executions, 512, "the execution sweep must be exhaustive");
    assert_eq!(
        property_a_checks,
        512 * REPLICAS * (STEPS + 2),
        "property (a) must be checked at every observable point"
    );
}

// ---------------------------------------------------------------------------
// Forward and backward non-interleaving.
// ---------------------------------------------------------------------------

/// One replica's contribution to a non-interleaving scenario: a heading
/// character and the passage that must stay glued to it.
struct Passage {
    /// The replica id / device number.
    replica: u8,
    /// The heading character, typed AFTER the passage in the backward case.
    heading: char,
    /// The passage.
    text: &'static str,
}

/// The scenarios of the paper's Table 1, as this file exercises them.
const TWO_WRITERS: [Passage; 2] = [
    Passage {
        replica: 1,
        heading: 'A',
        text: "aaa",
    },
    Passage {
        replica: 2,
        heading: 'B',
        text: "bbb",
    },
];

const THREE_WRITERS: [Passage; 3] = [
    Passage {
        replica: 1,
        heading: 'A',
        text: "aaa",
    },
    Passage {
        replica: 2,
        heading: 'B',
        text: "bbb",
    },
    Passage {
        replica: 3,
        heading: 'C',
        text: "cc",
    },
];

/// Assert every writer's block survived the merge contiguously, and that
/// nothing was lost or duplicated.
fn assert_blocks_are_contiguous(merged: &str, writers: &[Passage], expected_len: usize) {
    for writer in writers {
        let block = format!("{}{}", writer.heading, writer.text);
        assert!(
            merged.contains(&block),
            "interleaved: writer {}'s block {block:?} is not contiguous in {merged:?}",
            writer.replica
        );
    }
    assert_eq!(
        merged.chars().count(),
        expected_len,
        "the merge lost or duplicated characters: {merged:?}"
    );
}

/// A pure replica that has seen the seed, appended its passage at the END, and
/// then gone BACK to type its heading immediately before that passage.
///
/// This is the paper's Figure 2, the case RGA is proven to interleave.
fn backward_twin(seed: &Twin, writer: &Passage) -> Twin {
    let mut twin = Twin::new(u64::from(writer.replica));
    twin.receive_from(seed);
    for value in writer.text.chars() {
        let end = twin.len();
        let _ignored = twin.insert(end, value);
    }
    let _ignored = twin.insert(1, writer.heading);
    twin
}

/// A pure replica that typed its heading and passage LEFT TO RIGHT at the same
/// position everybody else is typing at.
///
/// This is the forward case, which RGA also satisfies: included so a
/// regression that broke only one direction cannot hide.
fn forward_twin(seed: &Twin, writer: &Passage) -> Twin {
    let mut twin = Twin::new(u64::from(writer.replica));
    twin.receive_from(seed);
    for (at, value) in (1..).zip(core::iter::once(writer.heading).chain(writer.text.chars())) {
        let _ignored = twin.insert(at, value);
    }
    twin
}

/// The shared seed `"S"`, authored by replica 0.
fn seed_twin() -> Twin {
    let mut seed = Twin::new(0);
    let _ignored = seed.insert(0, 'S');
    seed
}

/// Merge every twin into one and read the document back.
fn merged_text(twins: &[Twin]) -> String {
    let mut all = Twin::new(u64::MAX);
    for twin in twins {
        all.receive_from(twin);
    }
    all.text()
}

/// FORWARD non-interleaving on the pure algorithm: two replicas typing a run
/// left to right at the same position keep their runs contiguous.
#[test]
fn non_interleaving__forward_two_replicas_on_the_pure_algorithm() {
    let seed = seed_twin();
    let twins: Vec<Twin> = TWO_WRITERS
        .iter()
        .map(|writer| forward_twin(&seed, writer))
        .collect();
    assert_eq!(twins[0].text(), "SAaaa");
    assert_eq!(twins[1].text(), "SBbbb");

    let merged = merged_text(&twins);
    assert_eq!(
        merged,
        merged_text(&[twins[1].clone(), twins[0].clone()]),
        "the merge must be commutative"
    );
    assert_blocks_are_contiguous(&merged, &TWO_WRITERS, 9);
}

/// BACKWARD non-interleaving on the pure algorithm, two replicas: the paper's
/// Figure 2, and the case Table 1 records RGA as failing.
#[test]
fn non_interleaving__backward_two_replicas_on_the_pure_algorithm() {
    let seed = seed_twin();
    let twins: Vec<Twin> = TWO_WRITERS
        .iter()
        .map(|writer| backward_twin(&seed, writer))
        .collect();
    assert_eq!(twins[0].text(), "SAaaa");
    assert_eq!(twins[1].text(), "SBbbb");

    let merged = merged_text(&twins);
    assert_eq!(
        merged,
        merged_text(&[twins[1].clone(), twins[0].clone()]),
        "the merge must be commutative"
    );
    assert_blocks_are_contiguous(&merged, &TWO_WRITERS, 9);
}

/// BACKWARD non-interleaving on the pure algorithm, THREE concurrent
/// single-replica sessions. Table I's multi-replica column is a different
/// shape - see [`non_interleaving__backward_one_session_spanning_two_replicas`].
#[test]
fn non_interleaving__backward_three_replicas_on_the_pure_algorithm() {
    let seed = seed_twin();
    let twins: Vec<Twin> = THREE_WRITERS
        .iter()
        .map(|writer| backward_twin(&seed, writer))
        .collect();

    // Every delivery order of the three, to rule out an order-sensitive pass.
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let mut reference: Option<String> = None;
    for order in orders {
        let ordered: Vec<Twin> = order.iter().map(|r| twins[*r].clone()).collect();
        let merged = merged_text(&ordered);
        assert_blocks_are_contiguous(&merged, &THREE_WRITERS, 12);
        match &reference {
            None => reference = Some(merged),
            Some(expected) => assert_eq!(&merged, expected, "delivery order {order:?} diverged"),
        }
    }
}

/// Drive the backward scenario on a `FugueText` document and return the
/// merged text every replica converged on.
fn fugue_backward_merge(field: &str, writers: &[Passage]) -> String {
    let base = fugue_genesis(field, "S");
    let stores: Vec<Store> = writers.iter().map(|_| fork(&base)).collect();
    let mut deltas: Vec<Vec<u8>> = Vec::new();

    for (index, writer) in writers.iter().enumerate() {
        let dev = device(writer.replica);
        let replica = u64::from(writer.replica);
        // Append the passage at the end of the document...
        deltas.push(edit::<FugueText<MainStorage>>(&stores[index], dev, |doc| {
            let end = doc.len().expect("len should succeed");
            doc.insert_str_with_replica(end, replica, writer.text)
                .expect("append should succeed");
        }));
        // ... then go back and type the heading immediately before it.
        deltas.push(edit::<FugueText<MainStorage>>(&stores[index], dev, |doc| {
            doc.insert_str_with_replica(1, replica, &writer.heading.to_string())
                .expect("heading insert should succeed");
        }));
    }

    let mut converged: Option<String> = None;
    for (index, writer) in writers.iter().enumerate() {
        let dev = device(writer.replica);
        for delta in &deltas {
            land(&stores[index], dev, delta);
        }
        let text = fugue_text_in(&stores[index], dev);
        match &converged {
            None => converged = Some(text),
            Some(expected) => assert_eq!(&text, expected, "replicas did not converge"),
        }
    }
    converged.expect("at least one writer")
}

/// BACKWARD non-interleaving through the storage collection's REAL apply path,
/// two replicas.
#[test]
fn non_interleaving__backward_two_replicas_through_the_apply_path() {
    let merged = fugue_backward_merge("conformance_backward_two", &TWO_WRITERS);
    assert_blocks_are_contiguous(&merged, &TWO_WRITERS, 9);
}

/// BACKWARD non-interleaving through the storage collection's REAL apply path,
/// three replicas.
#[test]
fn non_interleaving__backward_three_replicas_through_the_apply_path() {
    let merged = fugue_backward_merge("conformance_backward_three", &THREE_WRITERS);
    assert_blocks_are_contiguous(&merged, &THREE_WRITERS, 12);
}

/// Drive ONE backward-typed session whose passage and heading carry ids from
/// two different replicas, concurrent with an ordinary single-replica writer,
/// and return the text both converge on.
fn multi_replica_backward_merge(field: &str, passage: u64, heading: u64) -> String {
    let base = fugue_genesis(field, "S");
    let (split, single) = (fork(&base), fork(&base));
    let (session, other) = (&TWO_WRITERS[0], &TWO_WRITERS[1]);
    let mut deltas: Vec<Vec<u8>> = Vec::new();

    let mut backward = |store: &Store, dev: [u8; 32], writer: &Passage, tail: u64, head: u64| {
        deltas.push(edit::<FugueText<MainStorage>>(store, dev, |doc| {
            let end = doc.len().expect("len should succeed");
            doc.insert_str_with_replica(end, tail, writer.text)
                .expect("append should succeed");
        }));
        deltas.push(edit::<FugueText<MainStorage>>(store, dev, |doc| {
            doc.insert_str_with_replica(1, head, &writer.heading.to_string())
                .expect("heading insert should succeed");
        }));
    };
    backward(&split, device(1), session, passage, heading);
    backward(
        &single,
        device(other.replica),
        other,
        u64::from(other.replica),
        u64::from(other.replica),
    );

    let mut converged: Option<String> = None;
    for (store, dev) in [(&split, device(1)), (&single, device(other.replica))] {
        for delta in &deltas {
            land(store, dev, delta);
        }
        let text = fugue_text_in(store, dev);
        match &converged {
            None => converged = Some(text),
            Some(expected) => assert_eq!(&text, expected, "replicas did not converge"),
        }
    }
    converged.expect("both writers wrote")
}

/// BACKWARD non-interleaving, MULTI-REPLICA - Table I's third anomaly column.
///
/// The three-writer cases above are three independent single-replica sessions
/// and do not reach this column: it needs ONE session spanning two replica ids,
/// which is what a user moving between devices produces. Both id orders are
/// exercised, because the column exists precisely because the ids need not line
/// up with the session.
#[test]
fn non_interleaving__backward_one_session_spanning_two_replicas() {
    for (field, passage, heading, expected) in [
        ("conformance_backward_split_low_first", 1, 4, "SAaaaBbbb"),
        ("conformance_backward_split_high_first", 4, 1, "SBbbbAaaa"),
    ] {
        let merged = multi_replica_backward_merge(field, passage, heading);
        assert_blocks_are_contiguous(&merged, &TWO_WRITERS, 9);
        assert_eq!(merged, expected, "session ids ({passage}, {heading})");
    }
}

/// FORWARD non-interleaving through the storage collection's REAL apply path.
#[test]
fn non_interleaving__forward_two_replicas_through_the_apply_path() {
    const FIELD: &str = "conformance_forward_two";
    let base = fugue_genesis(FIELD, "S");
    let stores: Vec<Store> = TWO_WRITERS.iter().map(|_| fork(&base)).collect();
    let mut deltas: Vec<Vec<u8>> = Vec::new();

    for (index, writer) in TWO_WRITERS.iter().enumerate() {
        let dev = device(writer.replica);
        let replica = u64::from(writer.replica);
        let run: String = core::iter::once(writer.heading)
            .chain(writer.text.chars())
            .collect();
        // Typed left to right at position 1: one call per character, each at
        // the position after the last.
        for (offset, value) in run.chars().enumerate() {
            deltas.push(edit::<FugueText<MainStorage>>(&stores[index], dev, |doc| {
                doc.insert_str_with_replica(1 + offset, replica, &value.to_string())
                    .expect("insert should succeed");
            }));
        }
    }

    for (index, writer) in TWO_WRITERS.iter().enumerate() {
        let dev = device(writer.replica);
        for delta in &deltas {
            land(&stores[index], dev, delta);
        }
        assert_blocks_are_contiguous(&fugue_text_in(&stores[index], dev), &TWO_WRITERS, 9);
    }
}

/// The paper's Figure 7, with `passage`-character passages: three replicas
/// concurrently insert `A`, `B` and `C` into an empty document, then r2 sees
/// `{A, B}` and types `Y`s between them while r3 sees `{A, C}` and types `X`s.
///
/// Every `Y` and `X` is a right descendant of `A` with a DIFFERENT right origin
/// (`B` versus `C`) - the one execution shape in which Fugue and FugueMax
/// disagree.
fn figure_7_merged(passage: u32) -> String {
    let mut r1 = FugueTree::new();
    let a = r1.insert(0, 'A', (1, 0)).unwrap();
    let mut r2 = FugueTree::new();
    let b = r2.insert(0, 'B', (2, 0)).unwrap();
    let mut r3 = FugueTree::new();
    let c = r3.insert(0, 'C', (3, 0)).unwrap();
    for node in [a, b, c] {
        assert_eq!((node.parent, node.side), (None, Side::R));
    }

    r2.integrate(a);
    assert_eq!(r2.values(), "AB");
    let ys: Vec<FugueNode> = (0..passage)
        .map(|k| r2.insert(1 + k as usize, 'Y', (2, 1 + k)).unwrap())
        .collect();

    r3.integrate(a);
    assert_eq!(r3.values(), "AC");
    let xs: Vec<FugueNode> = (0..passage)
        .map(|k| r3.insert(1 + k as usize, 'X', (3, 1 + k)).unwrap())
        .collect();

    // Both passages hang off A's right side, so the sibling comparator alone
    // decides their relative order and neither records a right origin.
    for head in [ys[0], xs[0]] {
        assert_eq!((head.parent, head.side), (Some((1, 0)), Side::R));
    }

    let all: Vec<FugueNode> = [a, b, c].into_iter().chain(ys).chain(xs).collect();
    let mut merged = FugueTree::new();
    let mut reversed = FugueTree::new();
    for node in &all {
        merged.integrate(*node);
    }
    for node in all.iter().rev() {
        reversed.integrate(*node);
    }
    assert_eq!(
        merged.values(),
        reversed.values(),
        "the merge must be commutative"
    );
    assert_eq!(merged.len(), all.len(), "the merge lost a character");
    merged.values()
}

/// FIGURE 7: right siblings are ordered by id, so this implementation is plain
/// Fugue and not FugueMax.
///
/// `Y` was typed immediately before `B`; only FugueMax keeps the two adjacent,
/// by ordering right siblings on the reverse of their right origins and
/// yielding `AXYBC`. The residual cost of plain Fugue is exactly that one lost
/// adjacency - neither order splits a passage. If this assertion ever changes,
/// the sibling comparator changed, and so did every stored document's text.
#[test]
fn figure_7__right_siblings_order_by_id_not_by_right_origin() {
    assert_eq!(figure_7_merged(1), "AYXBC");

    let multi = figure_7_merged(2);
    assert_eq!(multi, "AYYXXBC");
    assert!(
        multi.contains("YY") && multi.contains("XX"),
        "a typed passage was split: {multi:?}"
    );
}

// ---------------------------------------------------------------------------
// The same backward scenario against RGA: the defect Fugue fixes.
// ---------------------------------------------------------------------------

/// A pinned HLC, so the RGA scenario is deterministic: `CharId` is
/// `(timestamp, seq)` and the whole of RGA's tie-break is the timestamp.
fn pinned(time: u64) -> HybridTimestamp {
    let id = *HybridTimestamp::zero().get_id();
    HybridTimestamp::new(Timestamp::new(NTP64(time << 32), id))
}

/// The paper's Figure 2 against `ReplicatedGrowableArray`: RGA DOES interleave,
/// pinned to the exact text it produces.
///
/// This asserts the defect, not its absence - it is the control that keeps the
/// Fugue cases above meaningful. Both headings migrate to the front and both
/// passages follow, so neither writer's block survives. If it ever fails, RGA's
/// ordering changed and the comparison this collection exists to win is stale.
#[test]
fn rga_control__backward_two_replicas_interleave() {
    const FIELD: &str = "conformance_rga_backward";

    // Genesis: the shared seed "S", authored before either replica forks.
    let base = new_store();
    clear_pending_delta();
    env::with_runtime_env(env_for(&base, device(1)), || {
        let mut doc =
            Root::new(|| ReplicatedGrowableArray::<MainStorage>::new_with_field_name(FIELD));
        doc.insert_str_at_timestamp(0, pinned(1), "S")
            .expect("seed insert should succeed");
        doc.commit();
        let _ignored = env::take_last_artifact();
    });

    let stores: Vec<Store> = TWO_WRITERS.iter().map(|_| fork(&base)).collect();
    let mut deltas: Vec<Vec<u8>> = Vec::new();

    for (index, writer) in TWO_WRITERS.iter().enumerate() {
        let dev = device(writer.replica);
        let tick = u64::from(writer.replica);
        // Append the passage at the end...
        deltas.push(edit::<ReplicatedGrowableArray<MainStorage>>(
            &stores[index],
            dev,
            |doc| {
                let end = doc.len().expect("len should succeed");
                doc.insert_str_at_timestamp(end, pinned(10 + tick), writer.text)
                    .expect("append should succeed");
            },
        ));
        // ... then go back and type the heading immediately before it.
        deltas.push(edit::<ReplicatedGrowableArray<MainStorage>>(
            &stores[index],
            dev,
            |doc| {
                doc.insert_str_at_timestamp(1, pinned(20 + tick), &writer.heading.to_string())
                    .expect("heading insert should succeed");
            },
        ));
    }

    let mut converged: Option<String> = None;
    for (index, writer) in TWO_WRITERS.iter().enumerate() {
        let dev = device(writer.replica);
        for delta in &deltas {
            land(&stores[index], dev, delta);
        }
        let text =
            read_with::<ReplicatedGrowableArray<MainStorage>, _>(&stores[index], dev, |doc| {
                doc.get_text().expect("get_text should succeed")
            });
        match &converged {
            None => converged = Some(text),
            Some(expected) => assert_eq!(&text, expected, "RGA replicas did not converge"),
        }
    }

    // Every character survives, but both blocks are shredded: the assertion the
    // Fugue cases make (`assert_blocks_are_contiguous`) does not hold here.
    let merged = converged.expect("at least one writer");
    assert_eq!(merged, "SBAbbbaaa");
    assert_eq!(
        merged.chars().count(),
        9,
        "RGA lost or duplicated a character"
    );
    for writer in &TWO_WRITERS {
        let block = format!("{}{}", writer.heading, writer.text);
        assert!(
            !merged.contains(&block),
            "writer {}'s block {block:?} stayed contiguous under RGA, so this control \
             no longer exercises the anomaly",
            writer.replica
        );
    }
}
