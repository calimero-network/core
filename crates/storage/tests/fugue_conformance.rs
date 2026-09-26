//! Conformance to the external specification: Attiya et al.'s strong list
//! specification and the non-interleaving of Weidner, Gentle and Kleppmann,
//! *The Art of the Fugue* (arXiv 2305.00583), on the pure and the apply path.

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

/// A pure replica minting ids exactly as `FugueText` does: counter = nodes minted.
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

    fn insert(&mut self, pos: usize, value: char) -> FugueNode {
        let node = self.tree.insert(pos, value, (self.id, self.next)).unwrap();
        self.next += 1;
        node
    }

    fn delete(&mut self, pos: usize) -> RawId {
        self.tree.delete(pos).unwrap()
    }

    fn text(&self) -> String {
        self.tree.values()
    }

    fn len(&self) -> usize {
        self.tree.len()
    }

    fn receive_from(&mut self, other: &Self) {
        for node in other.tree.nodes().copied().collect::<Vec<_>>() {
            self.tree.integrate(node);
        }
    }

    fn received(&self) -> Vec<RawId> {
        self.tree.nodes().map(|node| node.id).collect()
    }

    fn deleted(&self) -> Vec<RawId> {
        self.tree
            .nodes()
            .filter(|node| node.value.is_none())
            .map(|node| node.id)
            .collect()
    }
}

/// Witness for the strong list spec's total order `<`: a shadow tree of every node created.
#[derive(Debug, Default)]
struct Elements {
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

    fn position(order: &[char], value: char) -> usize {
        order
            .iter()
            .position(|c| *c == value)
            .expect("every element appears in the total order")
    }
}

/// One `insert(i, x)` call: `values()` before it, the index, the new character.
#[derive(Debug)]
struct InsertCall {
    before: Vec<char>,
    index: usize,
    element: char,
}

/// Property (b): `a_0, …, a_{i-1} < e < a_i, …, a_{n-1}` in the total order `<`.
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

/// Property (a): the `<`-ordered restriction of the element set to what a replica has live.
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

#[derive(Clone, Copy, Debug)]
enum Act {
    InsFront,
    InsMid,
    InsEnd,
    DelFirst,
}

/// Front, middle and end are the three cases of Fugue's origin rule.
const ACTS: [Act; 4] = [Act::InsFront, Act::InsMid, Act::InsEnd, Act::DelFirst];

/// Element identities; unique per element, which is what makes `<` a string.
const POOL: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// The `index`-th point of the `steps`-step, `replicas`-replica script space.
fn script_of(index: usize, steps: usize, replicas: usize) -> Vec<(usize, Act)> {
    let radix = replicas * ACTS.len();
    (0..steps)
        .map(|step| {
            let digit = (index / radix.pow(u32::try_from(step).unwrap())) % radix;
            (digit % replicas, ACTS[digit / replicas])
        })
        .collect()
}

const fn position_for(act: Act, len: usize) -> usize {
    match act {
        Act::InsFront => 0,
        Act::InsMid => len / 2,
        Act::InsEnd => len,
        Act::DelFirst => 0,
    }
}

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

/// Element identity, which `FugueText` does not expose, is borrowed from a pure twin.
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

/// A writer's heading plus the passage that must stay glued to it.
struct Passage {
    replica: u8,
    heading: char,
    text: &'static str,
}

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

/// Appends the passage at the end, then goes back to type the heading before it.
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

/// Types heading and passage left to right at the position everybody types at.
fn forward_twin(seed: &Twin, writer: &Passage) -> Twin {
    let mut twin = Twin::new(u64::from(writer.replica));
    twin.receive_from(seed);
    for (at, value) in (1..).zip(core::iter::once(writer.heading).chain(writer.text.chars())) {
        let _ignored = twin.insert(at, value);
    }
    twin
}

fn seed_twin() -> Twin {
    let mut seed = Twin::new(0);
    let _ignored = seed.insert(0, 'S');
    seed
}

fn merged_text(twins: &[Twin]) -> String {
    let mut all = Twin::new(u64::MAX);
    for twin in twins {
        all.receive_from(twin);
    }
    all.text()
}

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

#[test]
fn non_interleaving__backward_three_replicas_on_the_pure_algorithm() {
    let seed = seed_twin();
    let twins: Vec<Twin> = THREE_WRITERS
        .iter()
        .map(|writer| backward_twin(&seed, writer))
        .collect();

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

fn fugue_backward_merge(field: &str, writers: &[Passage]) -> String {
    let base = fugue_genesis(field, "S");
    let stores: Vec<Store> = writers.iter().map(|_| fork(&base)).collect();
    let mut deltas: Vec<Vec<u8>> = Vec::new();

    for (index, writer) in writers.iter().enumerate() {
        let dev = device(writer.replica);
        let replica = u64::from(writer.replica);
        deltas.push(edit::<FugueText<MainStorage>>(&stores[index], dev, |doc| {
            let end = doc.len().expect("len should succeed");
            doc.insert_str_with_replica(end, replica, writer.text)
                .expect("append should succeed");
        }));
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

#[test]
fn non_interleaving__backward_two_replicas_through_the_apply_path() {
    let merged = fugue_backward_merge("conformance_backward_two", &TWO_WRITERS);
    assert_blocks_are_contiguous(&merged, &TWO_WRITERS, 9);
}

#[test]
fn non_interleaving__backward_three_replicas_through_the_apply_path() {
    let merged = fugue_backward_merge("conformance_backward_three", &THREE_WRITERS);
    assert_blocks_are_contiguous(&merged, &THREE_WRITERS, 12);
}

/// One backward session whose passage and heading carry different replica ids.
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

/// Figure 7: two passages, both right descendants of `A`, with different right origins.
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

/// Plain Fugue, not FugueMax: FugueMax would keep `Y` beside `B` and give `AXYBC`.
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

/// A pinned HLC: RGA's tie-break is the timestamp, so the scenario is deterministic.
fn pinned(time: u64) -> HybridTimestamp {
    let id = *HybridTimestamp::zero().get_id();
    HybridTimestamp::new(Timestamp::new(NTP64(time << 32), id))
}

/// Asserts the defect: were RGA to stop interleaving, the Fugue cases would pass vacuously.
#[test]
fn rga_control__backward_two_replicas_interleave() {
    const FIELD: &str = "conformance_rga_backward";

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
        deltas.push(edit::<ReplicatedGrowableArray<MainStorage>>(
            &stores[index],
            dev,
            |doc| {
                let end = doc.len().expect("len should succeed");
                doc.insert_str_at_timestamp(end, pinned(10 + tick), writer.text)
                    .expect("append should succeed");
            },
        ));
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
