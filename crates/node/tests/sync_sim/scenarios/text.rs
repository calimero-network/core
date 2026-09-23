//! Multi-replica `FugueText` scenarios over the real sync paths: edits travel
//! as real `StorageDelta` bytes through HashComparison or level-wise sessions.
//! Convergence is not the oracle - each scenario replays a pure `FugueTree`.

use std::collections::{BTreeMap, HashSet};
use std::slice;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId};
use calimero_storage::collections::fugue_text::{Anchor, Bias};
use calimero_storage::collections::rekey::{field_child_id, RekeyTarget};
use calimero_storage::collections::{
    Counter, FugueText, LwwRegister, MergeStrategy, Mergeable, Root, UnorderedMap,
};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::take_last_artifact;
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::MainStorage;
use calimero_storage::{register_rekey_if_supported, rekey_field_if_supported};

use crate::sync_sim::node::SimNode;
use crate::sync_sim::protocol::{execute_hash_comparison_sync, execute_level_wise_sync};
use crate::sync_sim::runtime::SimRng;

/// The single-document state these scenarios drive.
pub(crate) type Text = FugueText<MainStorage>;

pub(crate) const FIELD: &str = "sim_text_doc";
pub(crate) const CHAR_BASE: u32 = 0xE000; // private-use area, so every inserted character is unique
pub(crate) const CHAR_STRIDE: u32 = 0x400; // per-replica slice, so a stray character names its author
pub(crate) const PASSAGE_LEN: usize = 260; // longer than FugueText's 256-node run cap
pub(crate) const LOSS_RATE: f64 = 0.3;
pub(crate) const DUPLICATE_RATE: f64 = 0.25;
pub(crate) const EDIT_ROUNDS: usize = 4;
pub(crate) const SYNC_ROUNDS: usize = 8;
pub(crate) const DELIVERY_PASSES: usize = 64;
pub(crate) const HOSTILE_REPLICA: u64 = 7;

/// A pure-`FugueTree` mirror of one replica, minting ids as `FugueText` does.
pub(crate) struct Oracle {
    tree: FugueTree,
    replica: u64,
    next: u32,
}

impl Oracle {
    pub(crate) fn new(replica: u64) -> Self {
        Self {
            tree: FugueTree::new(),
            replica,
            next: 0,
        }
    }

    pub(crate) fn insert_str(&mut self, pos: usize, s: &str) {
        for (offset, content) in s.chars().enumerate() {
            let id = (self.replica, self.next);
            self.next += 1;
            let _ignored = self
                .tree
                .insert(pos + offset, content, id)
                .expect("caller clamps the insert position to the document");
        }
    }

    pub(crate) fn delete_range(&mut self, start: usize, end: usize) {
        let count = end.min(self.tree.len()).saturating_sub(start);
        for _ in 0..count {
            if self.tree.delete(start).is_err() {
                break;
            }
        }
    }

    pub(crate) fn snapshot(&self) -> BTreeMap<RawId, FugueNode> {
        self.tree.nodes().map(|node| (node.id, *node)).collect()
    }

    pub(crate) fn nodes(&self) -> Vec<FugueNode> {
        self.tree.nodes().copied().collect()
    }

    pub(crate) fn integrate_all(&mut self, nodes: &[FugueNode]) {
        for node in nodes {
            self.tree.integrate(*node);
        }
    }

    pub(crate) fn text(&self) -> String {
        self.tree.values()
    }

    pub(crate) fn len(&self) -> usize {
        self.tree.len()
    }

    /// The gap `anchor` names now, or `None` for a character this replica has
    /// not received. By linear scan: the oracle never borrows an index.
    pub(crate) fn resolve(&self, anchor: &Anchor) -> Option<usize> {
        let (wanted, bias) = match *anchor {
            Anchor::Start => return Some(0),
            Anchor::End => return Some(self.tree.len()),
            Anchor::Char { id, bias } => (id, bias),
        };
        let mut live = 0_usize;
        for id in self.tree.order().into_iter().flatten() {
            let Some(node) = self.tree.node(id) else {
                continue;
            };
            if id == wanted {
                return Some(live + usize::from(node.value.is_some() && bias == Bias::After));
            }
            live += usize::from(node.value.is_some());
        }
        None
    }
}

/// The oracle's model of one delta: nodes `after` added, plus ones it tombstoned.
pub(crate) fn changed(
    before: &BTreeMap<RawId, FugueNode>,
    after: &BTreeMap<RawId, FugueNode>,
) -> Vec<FugueNode> {
    after
        .iter()
        .filter(|(id, node)| before.get(*id).is_none_or(|old| old.value != node.value))
        .map(|(_, node)| *node)
        .collect()
}

/// Materialise the empty document, the way production `init` does.
pub(crate) fn seed_doc<T: BorshSerialize + BorshDeserialize>(
    node: &SimNode,
    init: impl FnOnce() -> T,
) {
    clear_pending_delta();
    node.storage().with_index(|| {
        let doc = Root::new(init);
        doc.commit();
        let _ignored = take_last_artifact();
    });
    let root = node.storage().root_id();
    let blocks = blocks_collection_ids(node);
    assert!(
        node.storage()
            .get_children(root)
            .iter()
            .any(|child| blocks.contains(&child.id())),
        "{}: blocks collection is not a child of the context root - collections::ROOT_ID was \
         frozen under a different context",
        node.id()
    );
}

pub(crate) fn edit<T: BorshSerialize + BorshDeserialize>(
    node: &SimNode,
    f: impl FnOnce(&mut Root<T>),
) -> Vec<u8> {
    clear_pending_delta();
    node.storage().with_index(|| {
        let mut state = Root::<T>::fetch().expect("state root should exist");
        f(&mut state);
        state.commit();
        take_last_artifact().expect("commit should emit a delta")
    })
}

/// Land `delta` the way the receive path does, action by action.
pub(crate) fn land(node: &SimNode, delta: &[u8]) {
    let actions = match borsh::from_slice::<StorageDelta>(delta).expect("delta should decode") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    node.storage().with_index(|| {
        for action in actions {
            if action.id().is_root() {
                continue;
            }
            Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                .expect("remote apply_action should succeed");
        }
    });
}

pub(crate) fn text_of(node: &SimNode) -> String {
    node.storage().with_index(|| {
        Root::<FugueText<MainStorage>>::fetch()
            .expect("document root should exist")
            .get_text()
            .expect("get_text should succeed")
    })
}

pub(crate) fn len_of(node: &SimNode) -> usize {
    node.storage().with_index(|| {
        Root::<FugueText<MainStorage>>::fetch()
            .expect("document root should exist")
            .len()
            .expect("len should succeed")
    })
}

pub(crate) fn read<T: BorshSerialize + BorshDeserialize, R>(
    node: &SimNode,
    f: impl FnOnce(&Root<T>) -> R,
) -> R {
    node.storage()
        .with_index(|| f(&Root::<T>::fetch().expect("state root should exist")))
}

/// Every document's block collection: nested `FugueText`s hang off the context root too.
pub(crate) fn blocks_collection_ids(node: &SimNode) -> Vec<Id> {
    let root = node.storage().root_id();
    let ids: Vec<Id> = node
        .storage()
        .get_children(root)
        .into_iter()
        .filter(|child| {
            node.storage()
                .get_index(child.id())
                .and_then(|index| index.metadata.crdt_type.clone())
                == Some(CrdtType::FugueText)
        })
        .map(|child| child.id())
        .collect();
    assert!(
        !ids.is_empty(),
        "{}: no FugueText collection under the root",
        node.id()
    );
    ids
}

pub(crate) fn block_count(node: &SimNode) -> usize {
    blocks_collection_ids(node)
        .into_iter()
        .map(|id| node.storage().get_children(id).len())
        .sum()
}

pub(crate) fn replica_of(node: &SimNode) -> u64 {
    let executor: [u8; 32] = *node.storage().executor_id().as_ref();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&executor[..8]);
    u64::from_be_bytes(head)
}

pub(crate) fn unique_chars(index: usize, from: usize, count: usize) -> String {
    let base = CHAR_BASE + CHAR_STRIDE * u32::try_from(index).expect("replica index fits u32");
    (0..count)
        .map(|i| {
            let offset = u32::try_from(from + i).expect("character index fits u32");
            assert!(
                offset < CHAR_STRIDE,
                "replica {index} exhausted its character slice"
            );
            char::from_u32(base + offset).expect("private-use code point is a valid char")
        })
        .collect()
}

/// A row that lost its `FugueTextBlock` tag is reconciled last-writer-wins,
/// which drops one side's edit silently rather than erroring.
pub(crate) fn assert_blocks_tagged(label: &str, node: &SimNode) {
    let blocks: Vec<_> = blocks_collection_ids(node)
        .into_iter()
        .flat_map(|id| node.storage().get_children(id))
        .collect();
    assert!(
        !blocks.is_empty(),
        "{label}: {} holds no block rows, so the tag check would pass vacuously",
        node.id()
    );
    for child in blocks {
        let index = node
            .storage()
            .get_index(child.id())
            .unwrap_or_else(|| panic!("{label}: {} block row has no index", node.id()));
        assert_eq!(
            index.metadata.crdt_type,
            Some(CrdtType::FugueTextBlock),
            "{label}: {} block row {:?} lost its CRDT tag",
            node.id(),
            child.id()
        );
    }
}

/// Every row reachable from the context root, paired with the tag that decides
/// how a conflicting copy of it is merged.
pub(crate) fn tagged_rows(node: &SimNode) -> BTreeMap<Id, Option<CrdtType>> {
    let mut out = BTreeMap::new();
    let mut frontier = vec![node.storage().root_id()];
    while let Some(id) = frontier.pop() {
        for child in node.storage().get_children(id) {
            let tag = node
                .storage()
                .get_index(child.id())
                .and_then(|index| index.metadata.crdt_type.clone());
            if out.insert(child.id(), tag).is_none() {
                frontier.push(child.id());
            }
        }
    }
    out
}

/// A text row that lost its `FugueTextBlock` tag reconciles last-writer-wins and
/// drops every node only the loser defines; a mark, property or block row that
/// GAINED a converging tag would be routed through the wrong arm instead.
/// Returns how many map rows it checked.
pub(crate) fn assert_rows_tagged(label: &str, node: &SimNode) -> usize {
    let rows = tagged_rows(node);
    let children_of = |wanted: &CrdtType| -> Vec<Id> {
        rows.iter()
            .filter(|(_, tag)| tag.as_ref() == Some(wanted))
            .flat_map(|(id, _)| node.storage().get_children(*id))
            .map(|child| child.id())
            .collect()
    };

    let text_rows = children_of(&CrdtType::FugueText);
    let map_rows = children_of(&CrdtType::UnorderedMap);
    let map_row_count = map_rows.len();
    assert!(
        !text_rows.is_empty(),
        "{label}: {} holds no text rows, so the tag check would pass vacuously",
        node.id()
    );
    assert!(
        !map_rows.is_empty(),
        "{label}: {} holds no map rows, so the tag check would pass vacuously",
        node.id()
    );

    for id in text_rows {
        assert_eq!(
            rows.get(&id).cloned().flatten(),
            Some(CrdtType::FugueTextBlock),
            "{label}: {} text row {id:?} lost its CRDT tag",
            node.id()
        );
    }
    for id in map_rows {
        let tag = rows.get(&id).cloned().flatten();
        // HashComparison carries an untagged leaf as a synthetic opaque
        // `LwwRegister`, which merges identically and is not a hash input.
        assert!(
            tag.is_none() || matches!(tag, Some(CrdtType::LwwRegister { .. })),
            "{label}: {} map row {id:?} is stamped {tag:?}, which routes it through a \
             converging merge arm it was never written for",
            node.id()
        );
    }
    map_row_count
}

pub(crate) fn assert_text_properties(
    label: &str,
    nodes: &[&SimNode],
    expected: &str,
    passages: &[String],
) {
    let first = nodes.first().expect("at least one replica");
    for node in nodes {
        let text = text_of(node);
        assert_eq!(
            text.chars().count(),
            expected.chars().count(),
            "{label}: {} holds {} characters, the oracle holds {}",
            node.id(),
            text.chars().count(),
            expected.chars().count()
        );
        assert!(
            text == expected,
            "{label}: {} text differs from the oracle",
            node.id()
        );
        assert_eq!(
            len_of(node),
            expected.chars().count(),
            "{label}: {} reports a length that disagrees with its own text",
            node.id()
        );

        for passage in passages {
            assert_eq!(
                text.matches(passage.as_str()).count(),
                1,
                "{label}: {} did not keep a typed passage contiguous and unique",
                node.id()
            );
        }

        let mut seen = HashSet::new();
        for ch in text.chars() {
            assert!(
                seen.insert(ch),
                "{label}: {} shows character U+{:04X} more than once",
                node.id(),
                ch as u32
            );
        }

        assert_blocks_tagged(label, node);
    }

    let root = first.root_hash();
    for node in nodes {
        assert_eq!(
            node.root_hash(),
            root,
            "{label}: {} merkle root differs",
            node.id()
        );
    }
}

pub(crate) fn context() -> ContextId {
    ContextId::from(SimNode::DEFAULT_CONTEXT_ID)
}

/// HashComparison is bidirectional, so both ends hold the union afterwards.
pub(crate) async fn pull(nodes: &mut [SimNode], i: usize, j: usize) {
    if i == j {
        return;
    }
    if i < j {
        let (left, right) = nodes.split_at_mut(j);
        execute_hash_comparison_sync(&mut left[i], &right[0])
            .await
            .expect("hash-comparison sync should succeed");
    } else {
        let (left, right) = nodes.split_at_mut(i);
        execute_hash_comparison_sync(&mut right[0], &left[j])
            .await
            .expect("hash-comparison sync should succeed");
    }
}

pub(crate) async fn converge_group(nodes: &mut [SimNode], group: &[usize]) -> bool {
    let equal = |nodes: &[SimNode]| {
        group
            .iter()
            .all(|&i| nodes[i].root_hash() == nodes[group[0]].root_hash())
    };
    for _ in 0..SYNC_ROUNDS {
        if equal(nodes) {
            return true;
        }
        for &i in group {
            for &j in group {
                pull(nodes, i, j).await;
            }
        }
    }
    equal(nodes)
}

pub(crate) fn union_oracles(oracles: &mut [Oracle], group: &[usize]) {
    let all: Vec<FugueNode> = group.iter().flat_map(|&i| oracles[i].nodes()).collect();
    for &i in group {
        oracles[i].integrate_all(&all);
    }
}

pub(crate) struct Pending {
    pub(crate) key: (usize, usize),
    /// Which document the delta edited; always 0 where the state holds one.
    pub(crate) doc: usize,
    pub(crate) bytes: Vec<u8>,
    pub(crate) nodes: Vec<FugueNode>,
    pub(crate) deps: HashSet<(usize, usize)>,
}

/// Drops stay queued for retry, so loss reorders delivery but never loses it.
pub(crate) fn delivery_pass(
    nodes: &[SimNode],
    integrate: &mut impl FnMut(usize, &Pending),
    deltas: &[Pending],
    queue: &mut Vec<(usize, usize)>,
    applied: &mut [HashSet<(usize, usize)>],
    rng: &mut SimRng,
) {
    rng.shuffle(queue);
    let mut retry = Vec::new();
    for (index, to) in queue.drain(..) {
        let delta = &deltas[index];
        if !delta.deps.is_subset(&applied[to]) || rng.bool_with_probability(LOSS_RATE) {
            retry.push((index, to));
            continue;
        }
        land(&nodes[to], &delta.bytes);
        integrate(to, delta);
        let _ignored = applied[to].insert(delta.key);
        if rng.bool_with_probability(DUPLICATE_RATE) {
            land(&nodes[to], &delta.bytes);
        }
    }
    *queue = retry;
}

pub(crate) fn random_edit(
    node: &SimNode,
    oracle: &mut Oracle,
    index: usize,
    minted: &mut usize,
    rng: &mut SimRng,
) -> Vec<u8> {
    let len = oracle.len();
    let choice = if len == 0 { 0 } else { rng.gen_range_usize(4) };
    match choice {
        0 | 1 => {
            let count = if choice == 0 {
                1
            } else {
                1 + rng.gen_range_usize(8)
            };
            let text = unique_chars(index, *minted, count);
            *minted += count;
            let pos = rng.gen_range_usize(len + 1);
            let bytes = edit::<Text>(node, |doc| {
                doc.insert_str(pos, &text).expect("insert");
            });
            oracle.insert_str(pos, &text);
            bytes
        }
        2 => {
            let pos = rng.gen_range_usize(len);
            let bytes = edit::<Text>(node, |doc| {
                doc.delete(pos).expect("delete");
            });
            oracle.delete_range(pos, pos + 1);
            bytes
        }
        _ => {
            let start = rng.gen_range_usize(len);
            let end = start + 1 + rng.gen_range_usize(4);
            let bytes = edit::<Text>(node, |doc| {
                doc.delete_range(start, end).expect("delete range");
            });
            oracle.delete_range(start, end);
            bytes
        }
    }
}

#[tokio::test]
async fn text_concurrent_edits_converge_under_lossy_delta_delivery() {
    for seed in 0..3_u64 {
        let label = format!("concurrent edits (seed {seed})");
        let count = 3 + usize::try_from(seed % 3).expect("modulus fits usize");
        let mut rng = SimRng::new(seed);

        let nodes: Vec<SimNode> = (0..count)
            .map(|i| SimNode::new_in_context(format!("t{i}"), context()))
            .collect();
        for node in &nodes {
            seed_doc(node, || Text::new_with_field_name(FIELD));
        }
        let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
        let mut minted = vec![0_usize; count];
        let mut applied: Vec<HashSet<(usize, usize)>> = vec![HashSet::new(); count];
        let mut deltas: Vec<Pending> = Vec::new();
        let mut queue: Vec<(usize, usize)> = Vec::new();

        for round in 0..=EDIT_ROUNDS {
            // Final round: every replica types one passage and nothing deletes
            // after it, so passage contiguity is not a race with a delete.
            let last = round == EDIT_ROUNDS;
            for author in 0..count {
                assert_eq!(
                    len_of(&nodes[author]),
                    oracles[author].len(),
                    "{label}: round {round} replica t{author} drifted from its oracle"
                );
                let before = oracles[author].snapshot();
                let bytes = if last {
                    let passage = unique_chars(author, minted[author], PASSAGE_LEN);
                    minted[author] += PASSAGE_LEN;
                    let pos = rng.gen_range_usize(oracles[author].len() + 1);
                    let bytes = edit::<Text>(&nodes[author], |doc| {
                        doc.insert_str(pos, &passage).expect("passage insert");
                    });
                    oracles[author].insert_str(pos, &passage);
                    bytes
                } else {
                    random_edit(
                        &nodes[author],
                        &mut oracles[author],
                        author,
                        &mut minted[author],
                        &mut rng,
                    )
                };

                let key = (author, deltas.len());
                deltas.push(Pending {
                    key,
                    doc: 0,
                    bytes,
                    nodes: changed(&before, &oracles[author].snapshot()),
                    deps: applied[author].clone(),
                });
                let _ignored = applied[author].insert(key);
                let index = deltas.len() - 1;
                queue.extend((0..count).filter(|&to| to != author).map(|to| (index, to)));
            }

            if !last {
                delivery_pass(
                    &nodes,
                    &mut |to, delta| oracles[to].integrate_all(&delta.nodes),
                    &deltas,
                    &mut queue,
                    &mut applied,
                    &mut rng,
                );
            }
        }

        for _ in 0..DELIVERY_PASSES {
            if queue.is_empty() {
                break;
            }
            delivery_pass(
                &nodes,
                &mut |to, delta| oracles[to].integrate_all(&delta.nodes),
                &deltas,
                &mut queue,
                &mut applied,
                &mut rng,
            );
        }
        assert!(queue.is_empty(), "{label}: delivery queue never drained");

        let passages: Vec<String> = (0..count)
            .map(|author| unique_chars(author, minted[author] - PASSAGE_LEN, PASSAGE_LEN))
            .collect();
        let refs: Vec<&SimNode> = nodes.iter().collect();
        assert_text_properties(&label, &refs, &oracles[0].text(), &passages);
    }
}

#[tokio::test]
async fn text_partition_heal_keeps_each_sides_passage_contiguous() {
    const GROUPS: [[usize; 2]; 2] = [[0, 1], [2, 3]];

    for seed in 0..2_u64 {
        let label = format!("partition heal (seed {seed})");
        let mut rng = SimRng::new(0x9A_0000 + seed);

        let mut nodes: Vec<SimNode> = (0..4)
            .map(|i| SimNode::new_in_context(format!("p{i}"), context()))
            .collect();
        for node in &nodes {
            seed_doc(node, || Text::new_with_field_name(FIELD));
        }
        let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
        let mut minted = [0_usize; 4];

        let base = unique_chars(0, 0, 24);
        minted[0] += 24;
        let _ignored = edit::<Text>(&nodes[0], |doc| {
            doc.insert_str(0, &base).expect("base insert");
        });
        oracles[0].insert_str(0, &base);
        assert!(
            converge_group(&mut nodes, &[0, 1, 2, 3]).await,
            "{label}: the base document did not reach every replica"
        );
        union_oracles(&mut oracles, &[0, 1, 2, 3]);

        for round in 0..EDIT_ROUNDS {
            for group in GROUPS {
                for author in group {
                    assert_eq!(
                        len_of(&nodes[author]),
                        oracles[author].len(),
                        "{label}: round {round} replica p{author} drifted from its oracle"
                    );
                    let _ignored = random_edit(
                        &nodes[author],
                        &mut oracles[author],
                        author,
                        &mut minted[author],
                        &mut rng,
                    );
                }
                assert!(
                    converge_group(&mut nodes, &group).await,
                    "{label}: round {round} group {group:?} did not converge while partitioned"
                );
                union_oracles(&mut oracles, &group);
            }
        }

        assert_ne!(
            nodes[0].root_hash(),
            nodes[2].root_hash(),
            "{label}: the two groups must still differ before the heal"
        );

        let mut passages = Vec::new();
        for group in GROUPS {
            let author = group[0];
            let passage = unique_chars(author, minted[author], PASSAGE_LEN);
            minted[author] += PASSAGE_LEN;
            let _ignored = edit::<Text>(&nodes[author], |doc| {
                doc.insert_str(0, &passage).expect("passage insert");
            });
            oracles[author].insert_str(0, &passage);
            passages.push(passage);
        }

        assert!(
            converge_group(&mut nodes, &[0, 1, 2, 3]).await,
            "{label}: the mesh did not converge after the heal"
        );
        union_oracles(&mut oracles, &[0, 1, 2, 3]);

        let refs: Vec<&SimNode> = nodes.iter().collect();
        assert_text_properties(&label, &refs, &oracles[0].text(), &passages);
    }
}

#[tokio::test]
async fn text_late_joiner_catches_up_via_hash_comparison() {
    const SEED: u64 = 0x7E_4700;
    let label = format!("hash-comparison joiner (seed {SEED})");
    let mut rng = SimRng::new(SEED);

    let mut nodes: Vec<SimNode> = ["hc0", "hc1", "hc2", "hc-cold", "hc-joiner"]
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes[..3] {
        seed_doc(node, || Text::new_with_field_name(FIELD));
    }
    seed_doc(&nodes[4], || Text::new_with_field_name(FIELD));

    let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
    let mut minted = vec![0_usize; nodes.len()];
    let mut passages = Vec::new();
    for author in 0..3 {
        let _ignored = random_edit(
            &nodes[author],
            &mut oracles[author],
            author,
            &mut minted[author],
            &mut rng,
        );
        let passage = unique_chars(author, minted[author], PASSAGE_LEN);
        minted[author] += PASSAGE_LEN;
        let _ignored = edit::<Text>(&nodes[author], |doc| {
            doc.insert_str(0, &passage).expect("passage insert");
        });
        oracles[author].insert_str(0, &passage);
        passages.push(passage);
    }
    assert!(
        converge_group(&mut nodes, &[0, 1, 2]).await,
        "{label}: the editing replicas did not converge"
    );
    union_oracles(&mut oracles, &[0, 1, 2]);

    for _ in 0..SYNC_ROUNDS {
        pull(&mut nodes, 3, 0).await;
    }
    let cold_root = nodes[3].storage().root_id();
    assert!(
        !nodes[3].storage().get_children(cold_root).is_empty(),
        "{label}: the cold joiner received no leaves at all"
    );
    assert!(
        nodes[3]
            .storage()
            .with_index(Root::<FugueText<MainStorage>>::fetch)
            .is_none(),
        "{label}: HashComparison now transfers the context-root entity, so a cold joiner can \
         read the document without running init - update this scenario's note"
    );

    assert!(
        converge_group(&mut nodes, &[0, 4]).await,
        "{label}: the seeded joiner did not catch up"
    );
    union_oracles(&mut oracles, &[0, 4]);

    assert_text_properties(
        &label,
        &[&nodes[0], &nodes[4]],
        &oracles[0].text(),
        &passages,
    );
}

#[tokio::test]
async fn text_late_joiner_catches_up_via_level_wise() {
    const SEED: u64 = 0x1E_4E00;
    let label = format!("level-wise joiner (seed {SEED})");
    let mut rng = SimRng::new(SEED);

    let mut nodes: Vec<SimNode> = ["lw-author", "lw-joiner"]
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes {
        seed_doc(node, || Text::new_with_field_name(FIELD));
    }
    let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
    let mut minted = 0_usize;

    for _ in 0..EDIT_ROUNDS {
        let _ignored = random_edit(&nodes[0], &mut oracles[0], 0, &mut minted, &mut rng);
    }
    let passage = unique_chars(0, minted, PASSAGE_LEN);
    let _ignored = edit::<Text>(&nodes[0], |doc| {
        doc.insert_str(0, &passage).expect("passage insert");
    });
    oracles[0].insert_str(0, &passage);

    let expected_blocks = block_count(&nodes[0]);
    assert!(
        expected_blocks > 1,
        "{label}: the author must hold several block rows for the skip check to bite"
    );

    for _ in 0..SYNC_ROUNDS {
        if nodes[1].root_hash() == nodes[0].root_hash() {
            break;
        }
        let (left, right) = nodes.split_at_mut(1);
        execute_level_wise_sync(&mut right[0], &left[0])
            .await
            .expect("level-wise sync should succeed");
    }
    union_oracles(&mut oracles, &[0, 1]);

    assert_eq!(
        block_count(&nodes[1]),
        expected_blocks,
        "{label}: level-wise skipped block rows - an untagged leaf is dropped, not merged"
    );
    assert_text_properties(
        &label,
        &[&nodes[0], &nodes[1]],
        &oracles[0].text(),
        &[passage],
    );
}

#[tokio::test]
async fn text_stale_and_forged_blocks_resolve_identically() {
    let label = "stale and forged blocks";

    let author = SimNode::new_in_context("stale-author", context());
    let receiver = SimNode::new_in_context("stale-receiver", context());
    seed_doc(&author, || Text::new_with_field_name(FIELD));
    seed_doc(&receiver, || Text::new_with_field_name(FIELD));

    let head = unique_chars(0, 0, 3);
    let tail = unique_chars(0, 3, 3);
    let old = edit::<Text>(&author, |doc| {
        doc.insert_str(0, &head).expect("insert head");
    });
    let new = edit::<Text>(&author, |doc| {
        doc.insert_str(3, &tail).expect("insert tail");
    });
    land(&receiver, &new);
    land(&receiver, &old);

    let run = format!("{head}{tail}");
    assert_text_properties(label, &[&author, &receiver], &run, slice::from_ref(&run));

    // Both writers mint the same block key, for different text of equal length.
    let p = SimNode::new_in_context("forge-p", context());
    let q = SimNode::new_in_context("forge-q", context());
    seed_doc(&p, || Text::new_with_field_name(FIELD));
    seed_doc(&q, || Text::new_with_field_name(FIELD));

    let p_text = unique_chars(0, 0, 4);
    let q_text = unique_chars(1, 0, 4);
    let from_p = edit::<Text>(&p, |doc| {
        doc.insert_str_with_replica(0, HOSTILE_REPLICA, &p_text)
            .expect("forged insert");
    });
    let from_q = edit::<Text>(&q, |doc| {
        doc.insert_str_with_replica(0, HOSTILE_REPLICA, &q_text)
            .expect("forged insert");
    });
    land(&p, &from_q);
    land(&q, &from_p);

    let winner = if q_text > p_text { q_text } else { p_text };
    assert_text_properties(label, &[&p, &q], &winner, slice::from_ref(&winner));
}

#[tokio::test]
async fn text_anchor_resolves_to_the_same_character_on_every_replica() {
    let mut nodes: Vec<SimNode> = (0..2)
        .map(|i| SimNode::new_in_context(format!("a{i}"), context()))
        .collect();
    for node in &nodes {
        seed_doc(node, || Text::new_with_field_name(FIELD));
    }
    let _typed = edit::<Text>(&nodes[0], |doc| {
        let _minted = doc.insert_str(0, "hello world").expect("seed text");
    });
    assert!(converge_group(&mut nodes, &[0, 1]).await);

    let anchors: Vec<Vec<u8>> = [(6, Bias::Before), (5, Bias::After), (1, Bias::Before)]
        .into_iter()
        .map(|(pos, bias)| {
            let anchor =
                read::<Text, _>(&nodes[0], |doc| doc.anchor_at(pos, bias).expect("anchor"));
            borsh::to_vec(&anchor).expect("anchor should encode")
        })
        .collect();

    let _a = edit::<Text>(&nodes[0], |doc| {
        let _undo = doc.insert_str(0, "AA").expect("insert");
    });
    let _b = edit::<Text>(&nodes[1], |doc| {
        let _undo = doc.insert_str(11, "BB").expect("insert");
    });
    let _d = edit::<Text>(&nodes[1], |doc| {
        let _undo = doc.delete_range(1, 2).expect("delete");
    });
    assert!(converge_group(&mut nodes, &[0, 1]).await);

    for node in &nodes {
        assert_eq!(text_of(node), "AAhllo worldBB", "{}", node.id());
        assert_blocks_tagged("anchor", node);
        let resolved: Vec<usize> = anchors
            .iter()
            .map(|bytes| {
                let anchor: Anchor = borsh::from_slice(bytes).expect("anchor should decode");
                read::<Text, _>(node, |doc| doc.resolve(&anchor).expect("resolve"))
            })
            .collect();
        assert_eq!(resolved, [7, 6, 3], "{}", node.id());
    }
}

// ---------------------------------------------------------------------------
// A `FugueText` stored as a collection value: the shape the document layer uses.

/// One document, hand-wired the way `#[derive(Mergeable)]` generates it - the
/// derive lives in `calimero-sdk-macros`, which this crate does not depend on.
#[derive(BorshSerialize, BorshDeserialize, Default)]
pub(crate) struct SimDoc {
    title: LwwRegister<String>,
    body: FugueText,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for SimDoc {
    const DISPATCHED: bool = false;
}

impl Mergeable for SimDoc {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.title.merge(&other.title);
        self.body.merge(&other.body)
    }
}

impl RekeyTarget for SimDoc {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.title, field_child_id(parent_id, "title"));
        rekey_field_if_supported!(&mut self.body, field_child_id(parent_id, "body"));
    }
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
pub(crate) struct SimDocs {
    docs: UnorderedMap<String, SimDoc>,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for SimDocs {
    const DISPATCHED: bool = false;
}

impl Mergeable for SimDocs {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.docs.merge(&other.docs)
    }
}

impl RekeyTarget for SimDocs {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.docs, field_child_id(parent_id, "docs"));
    }
}

pub(crate) const DOC_KEYS: [&str; 3] = ["notes", "draft", "todo"];
pub(crate) const AUTHORS: usize = 3;

/// The `docs` map is re-keyed the way `#[app::state]` re-keys a top-level field,
/// so every replica addresses the same entry for the same document name.
pub(crate) fn seed_docs(node: &SimNode) {
    register_rekey_if_supported!(SimDoc);
    clear_pending_delta();
    node.storage().with_index(|| {
        Root::new(|| {
            let mut state = SimDocs::default();
            state.docs.reassign_deterministic_id("docs");
            state
        })
        .commit();
        let _ignored = take_last_artifact();
    });
}

pub(crate) fn doc_text(node: &SimNode, key: &str) -> String {
    read::<SimDocs, _>(node, |app| {
        app.docs
            .get(key)
            .expect("map read should succeed")
            .map_or_else(String::new, |doc| {
                doc.body.get_text().expect("get_text should succeed")
            })
    })
}

/// One random edit in one document, mirrored into that document's oracle.
pub(crate) fn random_doc_edit(
    node: &SimNode,
    oracle: &mut Oracle,
    index: usize,
    minted: &mut usize,
    key: &str,
    rng: &mut SimRng,
) -> Vec<u8> {
    let len = oracle.len();
    let key = key.to_owned();
    if len == 0 || rng.gen_range_usize(4) != 0 {
        let count = 1 + rng.gen_range_usize(8);
        let text = unique_chars(index, *minted, count);
        *minted += count;
        let pos = rng.gen_range_usize(len + 1);
        let bytes = edit::<SimDocs>(node, |app| {
            let mut doc = app
                .docs
                .entry(key)
                .expect("entry")
                .or_default()
                .expect("or_default");
            let _minted = doc.body.insert_str(pos, &text).expect("insert");
        });
        oracle.insert_str(pos, &text);
        bytes
    } else {
        let start = rng.gen_range_usize(len);
        let end = start + 1 + rng.gen_range_usize(4);
        let bytes = edit::<SimDocs>(node, |app| {
            let mut doc = app
                .docs
                .entry(key)
                .expect("entry")
                .or_default()
                .expect("or_default");
            let _removed = doc.body.delete_range(start, end).expect("delete range");
        });
        oracle.delete_range(start, end);
        bytes
    }
}

/// `names[..AUTHORS]` edit three nested documents under lossy, reordered
/// delivery; any further name is a replica that has heard nothing yet.
pub(crate) fn edited_document_mesh(
    label: &str,
    names: &[&str],
    seed: u64,
) -> (Vec<SimNode>, Vec<Vec<Oracle>>) {
    let mut rng = SimRng::new(seed);
    let nodes: Vec<SimNode> = names
        .iter()
        .map(|name| SimNode::new_in_context((*name).to_owned(), context()))
        .collect();
    for node in &nodes {
        seed_docs(node);
    }

    // One oracle per (replica, document): each document mints its own counters.
    let mut oracles: Vec<Vec<Oracle>> = nodes
        .iter()
        .map(|n| {
            (0..DOC_KEYS.len())
                .map(|_| Oracle::new(replica_of(n)))
                .collect()
        })
        .collect();
    let mut minted = vec![vec![0_usize; DOC_KEYS.len()]; nodes.len()];
    let mut applied: Vec<HashSet<(usize, usize)>> = vec![HashSet::new(); nodes.len()];
    let mut deltas: Vec<Pending> = Vec::new();
    let mut queue: Vec<(usize, usize)> = Vec::new();

    for round in 0..=EDIT_ROUNDS {
        // Final round: every replica types one passage and nothing deletes after
        // it, so passage contiguity is not a race with a delete.
        let last = round == EDIT_ROUNDS;
        for author in 0..AUTHORS {
            let doc = rng.gen_range_usize(DOC_KEYS.len());
            let before = oracles[author][doc].snapshot();
            let bytes = if last {
                let passage = unique_chars(author, minted[author][doc], PASSAGE_LEN);
                minted[author][doc] += PASSAGE_LEN;
                let pos = rng.gen_range_usize(oracles[author][doc].len() + 1);
                let key = DOC_KEYS[doc].to_owned();
                let bytes = edit::<SimDocs>(&nodes[author], |app| {
                    let mut entry = app
                        .docs
                        .entry(key)
                        .expect("entry")
                        .or_default()
                        .expect("or_default");
                    let _minted = entry
                        .body
                        .insert_str(pos, &passage)
                        .expect("passage insert");
                });
                oracles[author][doc].insert_str(pos, &passage);
                bytes
            } else {
                random_doc_edit(
                    &nodes[author],
                    &mut oracles[author][doc],
                    author,
                    &mut minted[author][doc],
                    DOC_KEYS[doc],
                    &mut rng,
                )
            };

            let key = (author, deltas.len());
            deltas.push(Pending {
                key,
                doc,
                bytes,
                nodes: changed(&before, &oracles[author][doc].snapshot()),
                deps: applied[author].clone(),
            });
            let _ignored = applied[author].insert(key);
            let index = deltas.len() - 1;
            queue.extend(
                (0..AUTHORS)
                    .filter(|&to| to != author)
                    .map(|to| (index, to)),
            );
        }

        if !last {
            delivery_pass(
                &nodes,
                &mut |to, delta| oracles[to][delta.doc].integrate_all(&delta.nodes),
                &deltas,
                &mut queue,
                &mut applied,
                &mut rng,
            );
        }
    }

    for _ in 0..DELIVERY_PASSES {
        if queue.is_empty() {
            break;
        }
        delivery_pass(
            &nodes,
            &mut |to, delta| oracles[to][delta.doc].integrate_all(&delta.nodes),
            &deltas,
            &mut queue,
            &mut applied,
            &mut rng,
        );
    }
    assert!(queue.is_empty(), "{label}: delivery queue never drained");
    (nodes, oracles)
}

/// Equal Merkle roots, one tagged block collection per document, and each
/// document's text equal to the union of every replica's oracle for it.
pub(crate) fn assert_documents_converged(
    label: &str,
    nodes: &[SimNode],
    oracles: &mut [Vec<Oracle>],
) {
    let root = nodes[0].root_hash();
    for node in nodes {
        assert_eq!(
            node.root_hash(),
            root,
            "{label}: {} merkle root differs",
            node.id()
        );
        assert_eq!(
            blocks_collection_ids(node).len(),
            DOC_KEYS.len(),
            "{label}: {} does not hold one block collection per document",
            node.id()
        );
        assert_blocks_tagged(label, node);
    }

    for (doc, key) in DOC_KEYS.iter().enumerate() {
        let all: Vec<FugueNode> = (0..AUTHORS).flat_map(|i| oracles[i][doc].nodes()).collect();
        oracles[0][doc].integrate_all(&all);
        let expected = oracles[0][doc].text();
        assert!(
            !expected.is_empty(),
            "{label}: document {key} stayed empty, so the check is vacuous"
        );
        for node in nodes {
            assert_eq!(
                doc_text(node, key),
                expected,
                "{label}: {} disagrees with the oracle for document {key}",
                node.id()
            );
        }
    }
}

#[tokio::test]
async fn nested_text_documents_converge_under_lossy_delta_delivery() {
    const SEED: u64 = 0x_4E_57_ED;
    let label = format!("nested documents (seed {SEED})");
    let (nodes, mut oracles) = edited_document_mesh(&label, &["n0", "n1", "n2"], SEED);
    assert_documents_converged(&label, &nodes, &mut oracles);
}

pub(crate) const JOINER_SEED: u64 = 0x_4E_57_ED;

/// A replica that learns the documents from a peer instead of authoring them
/// reaches the same Merkle root. See `nested_collection_entity_data_reaches_a_joiner`
/// for the minimal case.
#[tokio::test]
async fn nested_text_documents_reach_a_hash_comparison_joiner() {
    let label = format!("nested joiner (seed {JOINER_SEED})");
    let (mut nodes, mut oracles) =
        edited_document_mesh(&label, &["n0", "n1", "n2", "n-hc"], JOINER_SEED);

    assert!(
        converge_group(&mut nodes, &[0, 1, 2, 3]).await,
        "{label}: the hash-comparison joiner did not catch up"
    );
    assert_documents_converged(&label, &nodes, &mut oracles);
    for node in &nodes {
        assert_every_indexed_entity_has_data(&label, node);
    }
}

/// Level-wise carries a collection container as an internal `LevelNode`, which
/// has no room for the container's own row and no `has_children` flag that would
/// let the initiator both apply a row and still descend. A joiner that catches up
/// this way rebuilds every container from its children's ancestor chains, so its
/// `own_hash` stays the sender's stale `full_hash` and the root never settles.
#[tokio::test]
#[ignore = "level-wise cannot carry a collection container's own row without a LevelNode wire field"]
async fn nested_text_documents_reach_a_level_wise_joiner() {
    let label = format!("level-wise joiner (seed {JOINER_SEED})");
    let (mut nodes, mut oracles) =
        edited_document_mesh(&label, &["n0", "n1", "n2", "n-lw"], JOINER_SEED);

    for _ in 0..SYNC_ROUNDS {
        if nodes[3].root_hash() == nodes[0].root_hash() {
            break;
        }
        let (left, right) = nodes.split_at_mut(3);
        execute_level_wise_sync(&mut right[0], &left[0])
            .await
            .expect("level-wise sync should succeed");
    }
    assert_documents_converged(&label, &nodes, &mut oracles);
    for node in &nodes {
        assert_every_indexed_entity_has_data(&label, node);
    }
}

/// An entity the Merkle index lists but whose `Key::Entry` row is absent hashes
/// off a stale `own_hash` and is dropped from any snapshot this node serves.
pub(crate) fn assert_every_indexed_entity_has_data(label: &str, node: &SimNode) {
    let mut pending = vec![node.storage().root_id()];
    while let Some(id) = pending.pop() {
        assert!(
            node.storage().get_entity_data(id).is_some(),
            "{label}: {} holds an index entry for {id} with no entity data row",
            node.id()
        );
        pending.extend(node.storage().get_children(id).into_iter().map(|c| c.id()));
    }
}

/// Not about text: HashComparison used to emit only true leaves
/// (`hash_comparison_protocol.rs`, `collect_leaves_recursive`), so a nested
/// collection a peer never created locally was rebuilt from its children's
/// `parent_id` links with NO entity data row. Its own hash, and therefore the
/// context root hash, could then never match the author's.
#[derive(BorshSerialize, BorshDeserialize, Default)]
pub(crate) struct NestedCounters {
    tallies: UnorderedMap<String, Counter>,
}

#[diagnostic::do_not_recommend]
impl MergeStrategy for NestedCounters {
    const DISPATCHED: bool = false;
}

impl Mergeable for NestedCounters {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.tallies.merge(&other.tallies)
    }
}

impl RekeyTarget for NestedCounters {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.tallies, field_child_id(parent_id, "tallies"));
    }
}

/// An author and a peer that only ever receives, with one nested `Counter`
/// under a map - the smallest state that puts a collection container below the
/// context root without the receiver ever creating it locally.
pub(crate) fn nested_counter_pair(names: [&str; 2]) -> Vec<SimNode> {
    let nodes: Vec<SimNode> = names
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes {
        clear_pending_delta();
        node.storage().with_index(|| {
            Root::new(|| {
                let mut state = NestedCounters::default();
                state.tallies.reassign_deterministic_id("tallies");
                state
            })
            .commit();
            let _ignored = take_last_artifact();
        });
    }
    let _typed = edit::<NestedCounters>(&nodes[0], |app| {
        let mut tally = app
            .tallies
            .entry("t".to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        tally.increment().expect("increment");
    });
    nodes
}

#[tokio::test]
async fn nested_collection_entity_data_reaches_a_joiner() {
    let mut nodes = nested_counter_pair(["nc-author", "nc-joiner"]);

    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "the joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("nested counter", &nodes[1]);
}

/// The joiner pulls first, so it materialises the container from the leaves'
/// ancestor chain before anyone pushes the container's own row. From then on
/// both sides hold the same children, so nothing but an explicit repair of the
/// container row can close the gap.
#[tokio::test]
async fn nested_collection_container_row_reaches_a_joiner_that_pulled_first() {
    let mut nodes = nested_counter_pair(["pf-author", "pf-joiner"]);

    pull(&mut nodes, 1, 0).await;
    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "the joiner that pulled first never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("pull-first", &nodes[1]);
}

/// A map value that holds both an inline field and a nested collection: the
/// inline bytes live in the entry's own row, so they must survive the same
/// catch-up that carries the nested collection.
#[tokio::test]
async fn inline_field_beside_a_nested_collection_reaches_a_joiner() {
    let mut nodes: Vec<SimNode> = ["if-author", "if-joiner"]
        .into_iter()
        .map(|name| SimNode::new_in_context(name, context()))
        .collect();
    for node in &nodes {
        seed_docs(node);
    }
    let _typed = edit::<SimDocs>(&nodes[0], |app| {
        let mut doc = app
            .docs
            .entry("notes".to_owned())
            .expect("entry")
            .or_default()
            .expect("or_default");
        doc.title.set("a title only the author typed".to_owned());
        let _minted = doc.body.insert_str(0, "hello").expect("insert");
    });

    pull(&mut nodes, 1, 0).await;
    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "the joiner never reached the author's Merkle root"
    );
    assert_every_indexed_entity_has_data("inline field", &nodes[1]);
    let title = read::<SimDocs, _>(&nodes[1], |app| {
        app.docs
            .get("notes")
            .expect("map read should succeed")
            .map_or_else(String::new, |doc| doc.title.get().clone())
    });
    assert_eq!(title, "a title only the author typed");
    assert_eq!(doc_text(&nodes[1], "notes"), "hello");
}
