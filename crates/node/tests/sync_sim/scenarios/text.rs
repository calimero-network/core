//! Multi-replica `FugueText` scenarios over the real sync paths: edits travel
//! as real `StorageDelta` bytes through HashComparison or level-wise sessions.
//! Convergence is not the oracle - each scenario replays a pure `FugueTree`.

use std::collections::{BTreeMap, HashSet};
use std::slice;

use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_storage::address::Id;
use calimero_storage::collections::fugue::{FugueNode, FugueTree, RawId};
use calimero_storage::collections::{FugueText, Root};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::take_last_artifact;
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::MainStorage;

use crate::sync_sim::node::SimNode;
use crate::sync_sim::protocol::{execute_hash_comparison_sync, execute_level_wise_sync};
use crate::sync_sim::runtime::SimRng;

const FIELD: &str = "sim_text_doc";
const CHAR_BASE: u32 = 0xE000; // private-use area, so every inserted character is unique
const CHAR_STRIDE: u32 = 0x400; // per-replica slice, so a stray character names its author
const PASSAGE_LEN: usize = 260; // longer than FugueText's 256-node run cap
const LOSS_RATE: f64 = 0.3;
const DUPLICATE_RATE: f64 = 0.25;
const EDIT_ROUNDS: usize = 4;
const SYNC_ROUNDS: usize = 8;
const DELIVERY_PASSES: usize = 64;
const HOSTILE_REPLICA: u64 = 7;

/// A pure-`FugueTree` mirror of one replica, minting ids as `FugueText` does.
struct Oracle {
    tree: FugueTree,
    replica: u64,
    next: u32,
}

impl Oracle {
    fn new(replica: u64) -> Self {
        Self {
            tree: FugueTree::new(),
            replica,
            next: 0,
        }
    }

    fn insert_str(&mut self, pos: usize, s: &str) {
        for (offset, content) in s.chars().enumerate() {
            let id = (self.replica, self.next);
            self.next += 1;
            let _ignored = self
                .tree
                .insert(pos + offset, content, id)
                .expect("caller clamps the insert position to the document");
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

    fn snapshot(&self) -> BTreeMap<RawId, FugueNode> {
        self.tree.nodes().map(|node| (node.id, *node)).collect()
    }

    fn nodes(&self) -> Vec<FugueNode> {
        self.tree.nodes().copied().collect()
    }

    fn integrate_all(&mut self, nodes: &[FugueNode]) {
        for node in nodes {
            self.tree.integrate(*node);
        }
    }

    fn text(&self) -> String {
        self.tree.values()
    }

    fn len(&self) -> usize {
        self.tree.len()
    }
}

/// The oracle's model of one delta: nodes `after` added, plus ones it tombstoned.
fn changed(
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
fn seed_doc(node: &SimNode) {
    clear_pending_delta();
    node.storage().with_index(|| {
        let doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(FIELD));
        doc.commit();
        let _ignored = take_last_artifact();
    });
    let root = node.storage().root_id();
    let blocks = blocks_collection_id(node);
    assert!(
        node.storage()
            .get_children(root)
            .iter()
            .any(|child| child.id() == blocks),
        "{}: blocks collection is not a child of the context root - collections::ROOT_ID was \
         frozen under a different context",
        node.id()
    );
}

fn edit(node: &SimNode, f: impl FnOnce(&mut Root<FugueText<MainStorage>>)) -> Vec<u8> {
    clear_pending_delta();
    node.storage().with_index(|| {
        let mut doc = Root::<FugueText<MainStorage>>::fetch().expect("document root should exist");
        f(&mut doc);
        doc.commit();
        take_last_artifact().expect("commit should emit a delta")
    })
}

/// Land `delta` the way the receive path does, action by action.
fn land(node: &SimNode, delta: &[u8]) {
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

fn text_of(node: &SimNode) -> String {
    node.storage().with_index(|| {
        Root::<FugueText<MainStorage>>::fetch()
            .expect("document root should exist")
            .get_text()
            .expect("get_text should succeed")
    })
}

fn len_of(node: &SimNode) -> usize {
    node.storage().with_index(|| {
        Root::<FugueText<MainStorage>>::fetch()
            .expect("document root should exist")
            .len()
            .expect("len should succeed")
    })
}

fn blocks_collection_id(node: &SimNode) -> Id {
    let root = node.storage().root_id();
    node.storage()
        .get_children(root)
        .into_iter()
        .find(|child| {
            node.storage()
                .get_index(child.id())
                .and_then(|index| index.metadata.crdt_type.clone())
                == Some(CrdtType::FugueText)
        })
        .map(|child| child.id())
        .unwrap_or_else(|| panic!("{}: no FugueText collection under the root", node.id()))
}

fn block_count(node: &SimNode) -> usize {
    node.storage()
        .get_children(blocks_collection_id(node))
        .len()
}

fn replica_of(node: &SimNode) -> u64 {
    let executor: [u8; 32] = *node.storage().executor_id().as_ref();
    let mut head = [0_u8; 8];
    head.copy_from_slice(&executor[..8]);
    u64::from_be_bytes(head)
}

fn unique_chars(index: usize, from: usize, count: usize) -> String {
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
fn assert_blocks_tagged(label: &str, node: &SimNode) {
    let blocks = node.storage().get_children(blocks_collection_id(node));
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

fn assert_text_properties(label: &str, nodes: &[&SimNode], expected: &str, passages: &[String]) {
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

fn context() -> ContextId {
    ContextId::from(SimNode::DEFAULT_CONTEXT_ID)
}

/// HashComparison is bidirectional, so both ends hold the union afterwards.
async fn pull(nodes: &mut [SimNode], i: usize, j: usize) {
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

async fn converge_group(nodes: &mut [SimNode], group: &[usize]) -> bool {
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

fn union_oracles(oracles: &mut [Oracle], group: &[usize]) {
    let all: Vec<FugueNode> = group.iter().flat_map(|&i| oracles[i].nodes()).collect();
    for &i in group {
        oracles[i].integrate_all(&all);
    }
}

struct Pending {
    key: (usize, usize),
    bytes: Vec<u8>,
    nodes: Vec<FugueNode>,
    deps: HashSet<(usize, usize)>,
}

/// Drops stay queued for retry, so loss reorders delivery but never loses it.
fn delivery_pass(
    nodes: &[SimNode],
    oracles: &mut [Oracle],
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
        oracles[to].integrate_all(&delta.nodes);
        let _ignored = applied[to].insert(delta.key);
        if rng.bool_with_probability(DUPLICATE_RATE) {
            land(&nodes[to], &delta.bytes);
        }
    }
    *queue = retry;
}

fn random_edit(
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
            let bytes = edit(node, |doc| {
                doc.insert_str(pos, &text).expect("insert");
            });
            oracle.insert_str(pos, &text);
            bytes
        }
        2 => {
            let pos = rng.gen_range_usize(len);
            let bytes = edit(node, |doc| {
                doc.delete(pos).expect("delete");
            });
            oracle.delete_range(pos, pos + 1);
            bytes
        }
        _ => {
            let start = rng.gen_range_usize(len);
            let end = start + 1 + rng.gen_range_usize(4);
            let bytes = edit(node, |doc| {
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
            seed_doc(node);
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
                    let bytes = edit(&nodes[author], |doc| {
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
                    &mut oracles,
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
                &mut oracles,
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
            seed_doc(node);
        }
        let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
        let mut minted = [0_usize; 4];

        let base = unique_chars(0, 0, 24);
        minted[0] += 24;
        let _ignored = edit(&nodes[0], |doc| {
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
            let _ignored = edit(&nodes[author], |doc| {
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
        seed_doc(node);
    }
    seed_doc(&nodes[4]);

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
        let _ignored = edit(&nodes[author], |doc| {
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
        seed_doc(node);
    }
    let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
    let mut minted = 0_usize;

    for _ in 0..EDIT_ROUNDS {
        let _ignored = random_edit(&nodes[0], &mut oracles[0], 0, &mut minted, &mut rng);
    }
    let passage = unique_chars(0, minted, PASSAGE_LEN);
    let _ignored = edit(&nodes[0], |doc| {
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
    seed_doc(&author);
    seed_doc(&receiver);

    let head = unique_chars(0, 0, 3);
    let tail = unique_chars(0, 3, 3);
    let old = edit(&author, |doc| {
        doc.insert_str(0, &head).expect("insert head");
    });
    let new = edit(&author, |doc| {
        doc.insert_str(3, &tail).expect("insert tail");
    });
    land(&receiver, &new);
    land(&receiver, &old);

    let run = format!("{head}{tail}");
    assert_text_properties(label, &[&author, &receiver], &run, slice::from_ref(&run));

    // Both writers mint the same block key, for different text of equal length.
    let p = SimNode::new_in_context("forge-p", context());
    let q = SimNode::new_in_context("forge-q", context());
    seed_doc(&p);
    seed_doc(&q);

    let p_text = unique_chars(0, 0, 4);
    let q_text = unique_chars(1, 0, 4);
    let from_p = edit(&p, |doc| {
        doc.insert_str_with_replica(0, HOSTILE_REPLICA, &p_text)
            .expect("forged insert");
    });
    let from_q = edit(&q, |doc| {
        doc.insert_str_with_replica(0, HOSTILE_REPLICA, &q_text)
            .expect("forged insert");
    });
    land(&p, &from_q);
    land(&q, &from_p);

    let winner = if q_text > p_text { q_text } else { p_text };
    assert_text_properties(label, &[&p, &q], &winner, slice::from_ref(&winner));
}
