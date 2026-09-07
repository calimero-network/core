//! Conformance to the EXTERNAL specification Tree-Fugue is proven against.
//!
//! Everything in the crate's own sweeps checks Fugue against Fugue: a model
//! oracle that is the same algorithm, so the two agree by construction. This
//! file checks it against statements made by somebody else — Weidner, Gentle
//! and Kleppmann, *The Art of the Fugue* (arXiv 2305.00583) — and the
//! specification that paper's Theorem 1 proves Tree-Fugue satisfies.
//!
//! # 1. Attiya et al.'s strong list specification (paper Theorem 1)
//!
//! For any execution there must exist a total order `<` on all list elements
//! across all replicas such that:
//!
//! * **(a)** at any time, `values()` on a replica returns the values of
//!   exactly those elements for which it has received an `insert` and not a
//!   `delete`, in the order `<`; and
//! * **(b)** if a replica's `values()` yields `[a_0, …, a_{n-1}]` just before
//!   `insert(i, x)` is called, then the inserted element `e` satisfies
//!   `a_0, …, a_{i-1} < e < a_i, …, a_{n-1}`.
//!
//! Both are tested directly, not weakened into "the replicas agree". `<` is
//! witnessed explicitly: every element is given a globally unique character,
//! and the order is read off a shadow tree built from every node the execution
//! ever created with nothing tombstoned. Property (b) is then a literal
//! bracketing assertion against that one order.
//!
//! # 2. Maximal non-interleaving (paper §2, Figure 2, Table 1)
//!
//! Concurrent sequences of insertions at the same position must be placed one
//! after another, not interleaved. Two directions, which are NOT equivalent:
//!
//! * **Forward** (left to right): each replica types a run left to right at
//!   the same position. RGA satisfies this.
//! * **Backward** (right to left): each replica types a passage, then moves
//!   the cursor back and types a heading before it — Figure 2. Table 1 records
//!   RGA as FAILING this, in both the single- and multi-replica forms, and
//!   Tree-Fugue as satisfying it.
//!
//! The backward scenario is therefore also written against
//! [`ReplicatedGrowableArray`] and marked `#[ignore]`: it documents the defect
//! `FugueText` exists to fix.
//!
//! # Layers
//!
//! Every property here is checked on the pure algorithm AND through the
//! storage collection's real apply path (`Interface::apply_action`), which is
//! the path production sync runs. Where the storage layer needs element
//! identity it gets it from a pure twin driven by the identical script, and
//! the twin's agreement with the document is asserted at every step, so the
//! identity is not assumed.

// The `subject__scenario` naming convention this crate uses in its tests.
#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId};
use calimero_storage::collections::{FugueText, ReplicatedGrowableArray, Root};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, NTP64};
use calimero_storage::store::{Key, MainStorage};

// ---------------------------------------------------------------------------
// Storage harness: two replicas, separate stores, reconciled by apply_action.
// ---------------------------------------------------------------------------

/// An in-memory main-storage backend owned by one replica.
type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

/// The NATIVE DEFAULT context. `collections::ROOT_ID` is a process-global
/// seeded from the first `context_id()` anything in the binary asks for, so a
/// test that installs another one poisons `Root::new` for the whole process.
const CONTEXT_ID: [u8; 32] = [236_u8; 32];

fn new_store() -> Store {
    Rc::new(RefCell::new(HashMap::new()))
}

/// A byte-for-byte copy of `store` — another replica bootstrapped from it.
fn fork(store: &Store) -> Store {
    Rc::new(RefCell::new(store.borrow().clone()))
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

/// Device id of replica `n`, distinct in the first 8 bytes (all `local_replica`
/// reads).
fn device(n: u8) -> [u8; 32] {
    let mut id = [n; 32];
    id[..8].copy_from_slice(&u64::from(n).to_be_bytes());
    id
}

/// Run `f` against the document in `store`, returning the delta the commit
/// emits.
fn edit<T: BorshSerialize + BorshDeserialize>(
    store: &Store,
    device: [u8; 32],
    f: impl FnOnce(&mut Root<T>),
) -> Vec<u8> {
    clear_pending_delta();
    env::with_runtime_env(env_for(store, device), || {
        let mut doc = Root::<T>::fetch().expect("document root should exist");
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

/// Read `store`'s document through `read`.
fn read_with<T: BorshSerialize + BorshDeserialize, R>(
    store: &Store,
    device: [u8; 32],
    read: impl FnOnce(&Root<T>) -> R,
) -> R {
    env::with_runtime_env(env_for(store, device), || {
        read(&Root::<T>::fetch().expect("document root should exist"))
    })
}

/// The `FugueText` document `store` holds.
fn fugue_text_in(store: &Store, device: [u8; 32]) -> String {
    read_with::<FugueText<MainStorage>, _>(store, device, |doc| {
        doc.get_text().expect("get_text should succeed")
    })
}

/// A genesis store holding a `FugueText` seeded with `seed`, authored under
/// replica 0 so neither writer's counter space starts used.
fn fugue_genesis(field: &str, seed: &str) -> Store {
    let store = new_store();
    clear_pending_delta();
    env::with_runtime_env(env_for(&store, device(1)), || {
        let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(field));
        doc.insert_str_with_replica(0, 0, seed)
            .expect("seed insert should succeed");
        doc.commit();
        let _ignored = env::take_last_artifact();
    });
    store
}

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

    /// Deliver every node `other` holds — the state-based join.
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
/// `<` written out. This is a *witness*, not a re-derivation — the order comes
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

/// The four-act alphabet: three insertion positions (front, middle, end — the
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
/// The document is driven by the identical script as a pure twin, and the two
/// are asserted equal at every step — so element identity, which `FugueText`
/// does not expose, is borrowed from the twin without being assumed. Property
/// (a) is then asserted against the DOCUMENT's own `get_text`, and property (b)
/// against the text the document showed just before each insert.
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
// Maximal non-interleaving.
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
/// This is the paper's Figure 2 — the case RGA is proven to interleave.
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
/// This is the forward case, which RGA also satisfies — included so a
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

/// BACKWARD non-interleaving on the pure algorithm, two replicas — the paper's
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

/// BACKWARD non-interleaving on the pure algorithm, THREE replicas — Table 1
/// records RGA as failing the multi-replica form too, so it gets its own case.
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

// ---------------------------------------------------------------------------
// The same backward scenario against RGA — the defect Fugue fixes.
// ---------------------------------------------------------------------------

/// A pinned HLC, so the RGA scenario is deterministic: `CharId` is
/// `(timestamp, seq)` and the whole of RGA's tie-break is the timestamp.
fn pinned(time: u64) -> HybridTimestamp {
    let id = *HybridTimestamp::zero().get_id();
    HybridTimestamp::new(Timestamp::new(NTP64(time << 32), id))
}

/// The paper's Figure 2, run against `ReplicatedGrowableArray` instead of
/// `FugueText`.
///
/// IGNORED ON PURPOSE. This is not a test of our code being right; it is the
/// executable record of the defect `FugueText` replaces RGA to fix. *The Art of
/// the Fugue*, Table 1, lists RGA as exhibiting backward interleaving in both
/// the single- and multi-replica forms. Run it with
/// `cargo test -p calimero-storage --test fugue_conformance -- --ignored` to
/// see the anomaly; if it ever PASSES, the scenario has stopped exercising the
/// anomaly and the Fugue tests above are weaker than they look.
#[test]
#[ignore = "documents RGA's backward-interleaving defect, which FugueText exists to fix"]
fn non_interleaving__backward_two_replicas_INTERLEAVES_UNDER_RGA() {
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

    // The same assertion the Fugue cases make. RGA is expected to fail it.
    assert_blocks_are_contiguous(&converged.expect("at least one writer"), &TWO_WRITERS, 9);
}
