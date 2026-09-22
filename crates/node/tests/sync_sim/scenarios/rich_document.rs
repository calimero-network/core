//! Multi-replica `RichDocument` scenarios over the real sync paths: block
//! structure and block bodies travel as real `StorageDelta` bytes through delta
//! delivery, HashComparison or a level-wise session.
//!
//! The character layer keeps `text.rs`'s pure-`FugueTree` oracle, one per block,
//! so passage contiguity and per-character identity are checked against
//! something independent. The block layer is checked by properties that do not
//! re-derive the read - every replica renders the same list, every block appears
//! once, and every row still carries the tag that routes it to the right merge.
//! Differentiating the read itself against a naive model is `calimero-storage`'s
//! `rich_document_fuzz`, and duplicating it here would duplicate its mistakes.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use calimero_primitives::crdt::CrdtType;
use calimero_storage::address::Id;
use calimero_storage::collections::{
    BlockId, BlockView, DefaultMarks, DeltaOp, RichDocument, Root,
};
use calimero_storage::delta::clear_pending_delta;
use calimero_storage::env::take_last_artifact;
use calimero_storage::store::MainStorage;

use crate::sync_sim::node::SimNode;
use crate::sync_sim::protocol::execute_level_wise_sync;
use crate::sync_sim::runtime::SimRng;
use crate::sync_sim::scenarios::text::{
    changed, context, converge_group, delivery_pass, edit, land, read, replica_of, unique_chars,
    Oracle, Pending, DELIVERY_PASSES, EDIT_ROUNDS, SYNC_ROUNDS,
};

type Doc = RichDocument<DefaultMarks, MainStorage>;

const FIELD: &str = "sim_rich_document";
const PASSAGE_LEN: usize = 18; // a document read is quadratic in the oracle, so keep it short
const SEED_BLOCKS: usize = 2; // blocks every replica starts from
const SEED_LEN: usize = 6; // characters in each seeded block
const KINDS: [&str; 3] = ["paragraph", "heading", "code-block"];
const KEYS: [&str; 2] = ["bold", "comment:a"]; // After and None expands
const EDITS_PER_ROUND: usize = 4; // per replica, before the round's delivery pass

/// One replica's oracles, one pure `FugueTree` per block body.
type Oracles = BTreeMap<BlockId, Oracle>;

fn blocks_of(node: &SimNode) -> Vec<BlockView> {
    read::<Doc, _>(node, |doc| doc.blocks().expect("blocks should be readable"))
}

fn text_of(view: &BlockView) -> String {
    view.spans.iter().map(|span| span.text.as_str()).collect()
}

/// Materialise the empty document the way production `init` does.
fn seed_document(node: &SimNode) {
    clear_pending_delta();
    node.storage().with_index(|| {
        let doc = Root::new(|| Doc::new_with_field_name(FIELD));
        doc.commit();
    });
    let _ignored = take_last_artifact();
}

/// Every row reachable from the context root, paired with the tag that decides
/// how a conflicting copy of it is merged.
fn tagged_rows(node: &SimNode) -> BTreeMap<Id, Option<CrdtType>> {
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
fn assert_rows_tagged(label: &str, node: &SimNode) {
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
}

/// The standing assertions every document scenario ends with.
fn assert_document_properties(
    label: &str,
    nodes: &[&SimNode],
    oracles: &Oracles,
    passages: &[String],
) {
    let first = nodes.first().expect("at least one replica");
    let reference = blocks_of(first);
    let root = first.root_hash();
    assert!(
        !reference.is_empty(),
        "{label}: the document holds no blocks, so every assertion below is vacuous"
    );

    for node in nodes {
        let views = blocks_of(node);
        assert_eq!(
            views,
            reference,
            "{label}: {} renders a different block list",
            node.id()
        );

        let ids: BTreeSet<BlockId> = views.iter().map(|view| view.id).collect();
        assert_eq!(
            ids.len(),
            views.len(),
            "{label}: {} rendered a block more than once",
            node.id()
        );

        let mut seen = HashSet::new();
        for view in &views {
            let text = text_of(view);
            if let Some(oracle) = oracles.get(&view.id) {
                assert_eq!(
                    text,
                    oracle.text(),
                    "{label}: {} block {:?} differs from the oracle",
                    node.id(),
                    view.id
                );
            }
            assert_eq!(
                text.chars().count(),
                view.spans
                    .iter()
                    .map(|span| span.text.chars().count())
                    .sum::<usize>(),
                "{label}: {} spans do not spell block {:?}",
                node.id(),
                view.id
            );
            for ch in text.chars() {
                assert!(
                    seen.insert(ch),
                    "{label}: {} shows character U+{:04X} more than once",
                    node.id(),
                    ch as u32
                );
            }
        }

        for passage in passages {
            let holding = views
                .iter()
                .filter(|view| text_of(view).contains(passage.as_str()))
                .count();
            assert_eq!(
                holding,
                1,
                "{label}: {} did not keep a typed passage contiguous inside one block",
                node.id()
            );
        }

        assert_rows_tagged(label, node);
        assert_eq!(
            node.root_hash(),
            root,
            "{label}: {} merkle root differs",
            node.id()
        );
    }
}

fn retain(count: usize) -> DeltaOp {
    DeltaOp::Retain {
        retain: count,
        attributes: None,
    }
}

fn insert(text: &str) -> DeltaOp {
    DeltaOp::Insert {
        insert: text.to_owned(),
        attributes: None,
    }
}

/// Type `text` into `block` at `pos`, keeping that block's oracle in step.
fn type_into(
    node: &SimNode,
    oracles: &mut Oracles,
    block: BlockId,
    pos: usize,
    text: &str,
) -> Vec<u8> {
    let bytes = edit::<Doc>(node, |doc| {
        let _undo = doc
            .apply_delta(block, &[retain(pos), insert(text)])
            .expect("typing should succeed");
    });
    oracle_for(oracles, node, block).insert_str(pos, text);
    bytes
}

/// One random edit: type, delete, format, or change a block's structure.
fn random_edit(
    node: &SimNode,
    oracles: &mut Oracles,
    run: &mut Run,
    index: usize,
    minted: &mut usize,
    rng: &mut SimRng,
) -> (usize, Vec<u8>) {
    let views = blocks_of(node);
    let view = &views[rng.gen_range_usize(views.len())];
    let block = view.id;
    let len = text_of(view).chars().count();

    let bytes = match rng.gen_range_usize(8) {
        0 if len > 1 => {
            let start = rng.gen_range_usize(len - 1);
            let count = 1 + rng.gen_range_usize((len - start).min(3));
            let bytes = edit::<Doc>(node, |doc| {
                let _undo = doc
                    .apply_delta(block, &[retain(start), DeltaOp::Delete { delete: count }])
                    .expect("delete should succeed");
            });
            oracle_for(oracles, node, block).delete_range(start, start + count);
            bytes
        }
        1 if len > 1 => {
            let start = rng.gen_range_usize(len - 1);
            let end = start + 1 + rng.gen_range_usize(len - start - 1);
            let key = KEYS[rng.gen_range_usize(KEYS.len())];
            edit::<Doc>(node, |doc| {
                let _id = doc
                    .mark(block, start, end, key, Some("true"))
                    .expect("mark should succeed");
            })
        }
        2 => edit::<Doc>(node, |doc| {
            doc.set_kind(block, KINDS[rng.gen_range_usize(KINDS.len())])
                .expect("set_kind should succeed");
        }),
        3 => edit::<Doc>(node, |doc| {
            doc.set_depth(
                block,
                u8::try_from(rng.gen_range_usize(3)).expect("fits u8"),
            )
            .expect("set_depth should succeed");
        }),
        4 if views.len() > SEED_BLOCKS => edit::<Doc>(node, |doc| {
            doc.move_block(block, Some(views[0].id))
                .expect("move_block should succeed");
        }),
        5 | 6 if !run.protected.contains(&block) && views.len() > SEED_BLOCKS => {
            let _new = run.tombstoned.insert(block);
            edit::<Doc>(node, |doc| {
                let _was = doc
                    .delete_block(block)
                    .expect("delete_block should succeed");
            })
        }
        // One transaction, so the new block and its first characters travel
        // as one delta rather than as two that can be delivered apart.
        _ => {
            let length = 1 + rng.gen_range_usize(3);
            let text = unique_chars(index, *minted, length);
            *minted += length;
            let mut created = BlockId((0, 0));
            let bytes = edit::<Doc>(node, |doc| {
                created = doc
                    .insert_block(Some(block), KINDS[rng.gen_range_usize(KINDS.len())], 0)
                    .expect("insert_block should succeed");
                let _undo = doc
                    .apply_delta(created, &[insert(&text)])
                    .expect("typing should succeed");
            });
            let mut oracle = Oracle::new(replica_of(node));
            oracle.insert_str(0, &text);
            let _old = oracles.insert(created, oracle);
            return (slot_of(&mut run.registry, created), bytes);
        }
    };
    (slot_of(&mut run.registry, block), bytes)
}

/// The bookkeeping one run carries across every replica.
struct Run {
    /// Every block ever created, in creation order. A queued delta names its
    /// block by this index, which is the same number on every receiver; a
    /// per-replica map would not, since replicas hold different block sets
    /// until the queue drains.
    registry: Vec<BlockId>,
    /// The seeded blocks, which nothing deletes, so a typed passage has
    /// somewhere to survive.
    protected: BTreeSet<BlockId>,
    /// Every block any replica tombstoned; none of them may render again.
    tombstoned: BTreeSet<BlockId>,
}

/// A replica-independent index for a block.
fn slot_of(registry: &mut Vec<BlockId>, block: BlockId) -> usize {
    registry
        .iter()
        .position(|id| *id == block)
        .unwrap_or_else(|| {
            registry.push(block);
            registry.len() - 1
        })
}

/// The oracle for a block this replica may have learned about from a peer.
fn oracle_for<'a>(oracles: &'a mut Oracles, node: &SimNode, block: BlockId) -> &'a mut Oracle {
    oracles
        .entry(block)
        .or_insert_with(|| Oracle::new(replica_of(node)))
}

/// Seed every replica from one authored document, so they share something to
/// disagree about rather than several disjoint ones.
fn seeded_mesh(names: &[String]) -> (Vec<SimNode>, Vec<Oracles>, Vec<usize>, Vec<BlockId>) {
    let nodes: Vec<SimNode> = names
        .iter()
        .map(|name| SimNode::new_in_context(name.clone(), context()))
        .collect();
    for node in &nodes {
        seed_document(node);
    }
    let mut oracles: Vec<Oracles> = (0..nodes.len()).map(|_| Oracles::new()).collect();
    let mut minted = vec![0_usize; nodes.len()];
    let mut blocks = Vec::new();
    let mut deltas = Vec::new();

    for slot in 0..SEED_BLOCKS {
        let after = blocks.last().copied();
        let text = unique_chars(0, minted[0], SEED_LEN);
        minted[0] += SEED_LEN;
        let mut created = BlockId((0, 0));
        let bytes = edit::<Doc>(&nodes[0], |doc| {
            created = doc
                .insert_block(after, KINDS[slot % KINDS.len()], 0)
                .expect("seed insert_block should succeed");
            let _undo = doc
                .apply_delta(created, &[insert(&text)])
                .expect("seed typing should succeed");
        });
        blocks.push(created);
        deltas.push(bytes);

        let mut authored = Oracle::new(replica_of(&nodes[0]));
        authored.insert_str(0, &text);
        // Every other replica INTEGRATES the authored nodes rather than
        // re-typing them: an oracle mints ids under its own replica, so a
        // replay would collide with what the author already minted.
        let seeded_nodes = authored.nodes();
        let _old = oracles[0].insert(created, authored);
        for (index, oracle) in oracles.iter_mut().enumerate().skip(1) {
            let mut mirror = Oracle::new(replica_of(&nodes[index]));
            mirror.integrate_all(&seeded_nodes);
            let _old = oracle.insert(created, mirror);
        }
    }

    for node in nodes.iter().skip(1) {
        for bytes in &deltas {
            land(node, bytes);
        }
    }
    (nodes, oracles, minted, blocks)
}

#[tokio::test]
async fn rich_document_converges_under_lossy_delta_delivery() {
    for seed in 0..2_u64 {
        let label = format!("rich document lossy delivery (seed {seed})");
        let count = 3 + usize::try_from(seed % 3).expect("modulus fits usize");
        let mut rng = SimRng::new(0x_d0_c5_00 + seed);

        let names: Vec<String> = (0..count).map(|i| format!("d{i}")).collect();
        let (nodes, mut oracles, mut minted, seeded) = seeded_mesh(&names);
        let mut run = Run {
            registry: seeded.clone(),
            protected: seeded.iter().copied().collect(),
            tombstoned: BTreeSet::new(),
        };
        let mut applied: Vec<HashSet<(usize, usize)>> = vec![HashSet::new(); count];
        let mut deltas: Vec<Pending> = Vec::new();
        let mut queue: Vec<(usize, usize)> = Vec::new();

        for round in 0..=EDIT_ROUNDS {
            // The final round types one passage per replica with nothing
            // deleting after it, so contiguity is not racing a delete.
            let last = round == EDIT_ROUNDS;
            for author in 0..count {
                for _ in 0..if last { 1 } else { EDITS_PER_ROUND } {
                    let target = seeded[author % seeded.len()];
                    let before = oracles[author]
                        .get(&target)
                        .expect("a seeded oracle")
                        .snapshot();
                    let (slot, bytes) = if last {
                        let passage = unique_chars(author, minted[author], PASSAGE_LEN);
                        minted[author] += PASSAGE_LEN;
                        let pos = oracles[author].get(&target).expect("a seeded oracle").len();
                        let bytes =
                            type_into(&nodes[author], &mut oracles[author], target, pos, &passage);
                        (slot_of(&mut run.registry, target), bytes)
                    } else {
                        random_edit(
                            &nodes[author],
                            &mut oracles[author],
                            &mut run,
                            author,
                            &mut minted[author],
                            &mut rng,
                        )
                    };

                    let block = run.registry[slot];
                    let after = oracles[author]
                        .get(&block)
                        .map(Oracle::snapshot)
                        .unwrap_or_default();
                    let nodes_changed = if block == target {
                        changed(&before, &after)
                    } else {
                        after.values().copied().collect()
                    };

                    let key = (author, deltas.len());
                    deltas.push(Pending {
                        key,
                        doc: slot,
                        bytes,
                        nodes: nodes_changed,
                        deps: applied[author].clone(),
                    });
                    let _ignored = applied[author].insert(key);
                    let index = deltas.len() - 1;
                    queue.extend((0..count).filter(|&to| to != author).map(|to| (index, to)));
                }
            }

            if !last {
                drain_once(
                    &nodes,
                    &mut oracles,
                    &run.registry,
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
            drain_once(
                &nodes,
                &mut oracles,
                &run.registry,
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
        assert_document_properties(&label, &refs, &oracles[0], &passages);
        assert!(
            !run.tombstoned.is_empty(),
            "{label}: no block was ever deleted, so the tombstone check is vacuous"
        );
        for node in &refs {
            for view in blocks_of(node) {
                assert!(
                    !run.tombstoned.contains(&view.id),
                    "{label}: {} rendered the tombstoned block {:?}",
                    node.id(),
                    view.id
                );
            }
        }
    }
}

fn drain_once(
    nodes: &[SimNode],
    oracles: &mut [Oracles],
    registry: &[BlockId],
    deltas: &[Pending],
    queue: &mut Vec<(usize, usize)>,
    applied: &mut [HashSet<(usize, usize)>],
    rng: &mut SimRng,
) {
    delivery_pass(
        nodes,
        &mut |to, delta| {
            let block = registry[delta.doc];
            oracle_for(&mut oracles[to], &nodes[to], block).integrate_all(&delta.nodes);
        },
        deltas,
        queue,
        applied,
        rng,
    );
}

/// Both sides insert a block at the same spot, and one moves a block while the
/// other edits its body. After the heal both inserts survive, the moved block
/// appears once, and the edit is not lost.
#[tokio::test]
async fn rich_document_partition_insert_and_move_versus_edit() {
    let label = "partition: insert, move and edit";
    let names: Vec<String> = ["pd0", "pd1"].iter().map(|n| (*n).to_owned()).collect();
    let (mut nodes, mut oracles, mut minted, seeded) = seeded_mesh(&names);
    let (anchor, moved) = (seeded[0], seeded[1]);

    let mut created = [BlockId((0, 0)); 2];
    for side in 0..2 {
        let text = unique_chars(side, minted[side], 4);
        minted[side] += 4;
        let _bytes = edit::<Doc>(&nodes[side], |doc| {
            created[side] = doc
                .insert_block(Some(anchor), "paragraph", 0)
                .expect("insert_block should succeed");
            let _undo = doc
                .apply_delta(created[side], &[insert(&text)])
                .expect("typing should succeed");
        });
        oracle_for(&mut oracles[side], &nodes[side], created[side]).insert_str(0, &text);
    }

    let _moved = edit::<Doc>(&nodes[0], |doc| {
        doc.move_block(moved, None).expect("move should succeed");
    });
    let typed = unique_chars(1, minted[1], 5);
    minted[1] += 5;
    let _edit = type_into(&nodes[1], &mut oracles[1], moved, 0, &typed);

    assert_ne!(
        nodes[0].root_hash(),
        nodes[1].root_hash(),
        "{label}: the two sides must differ before the heal"
    );
    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "{label}: the mesh did not converge after the heal"
    );

    let views = blocks_of(&nodes[0]);
    assert_eq!(
        views.len(),
        SEED_BLOCKS + 2,
        "{label}: an independently inserted block was lost: {views:?}"
    );
    assert_eq!(
        views.iter().filter(|view| view.id == moved).count(),
        1,
        "{label}: the moved block appears more than once"
    );
    assert_eq!(
        views[0].id, moved,
        "{label}: the move did not take effect on the healed document"
    );
    assert!(
        text_of(&views[0]).contains(&typed),
        "{label}: the concurrent body edit was lost by the move"
    );
    assert_eq!(nodes[0].root_hash(), nodes[1].root_hash());
    for node in &nodes {
        assert_rows_tagged(label, node);
    }
}

/// A joiner that catches up over HashComparison must end byte-identical, with
/// every container row the author holds.
#[tokio::test]
async fn rich_document_reaches_a_hash_comparison_joiner() {
    let label = "hash-comparison joiner";
    let mut nodes: Vec<SimNode> = ["hd0", "hd1"]
        .iter()
        .map(|name| SimNode::new_in_context((*name).to_owned(), context()))
        .collect();
    for node in &nodes {
        seed_document(node);
    }
    let mut oracles = Oracles::new();

    let mut blocks = Vec::new();
    for slot in 0..3 {
        let after = blocks.last().copied();
        let mut created = BlockId((0, 0));
        let _bytes = edit::<Doc>(&nodes[0], |doc| {
            created = doc
                .insert_block(after, KINDS[slot % KINDS.len()], 0)
                .expect("insert_block should succeed");
            doc.set_attr(created, "align", Some("center"))
                .expect("set_attr should succeed");
        });
        let _old = oracles.insert(created, Oracle::new(replica_of(&nodes[0])));
        blocks.push(created);
    }
    let passage = unique_chars(0, 0, PASSAGE_LEN);
    let _typed = type_into(&nodes[0], &mut oracles, blocks[1], 0, &passage);
    let _formatted = edit::<Doc>(&nodes[0], |doc| {
        let _id = doc
            .mark(blocks[1], 0, 6, "bold", Some("true"))
            .expect("mark should succeed");
    });

    let author_rows: BTreeSet<Id> = tagged_rows(&nodes[0]).into_keys().collect();
    // Bidirectional, as every nested-container joiner test here is: a container
    // the joiner never created locally is rebuilt from its children's ancestor
    // chain, and only the other end's session carries the container's own row.
    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "{label}: the joiner never reached the author's Merkle root"
    );
    let joiner_rows: BTreeSet<Id> = tagged_rows(&nodes[1]).into_keys().collect();
    assert!(
        author_rows.is_subset(&joiner_rows),
        "{label}: the joiner is missing container rows: {:?}",
        author_rows.difference(&joiner_rows).collect::<Vec<_>>()
    );

    let refs: Vec<&SimNode> = nodes.iter().collect();
    assert_document_properties(label, &refs, &oracles, &[passage]);
}

/// The level-wise path hits the same wall the nested text scenario documents:
/// a collection container has no room for its own row on that wire, so a joiner
/// rebuilds it from its children and its `own_hash` never settles.
#[tokio::test]
#[ignore = "level-wise cannot carry a collection container's own row without a LevelNode wire field"]
async fn rich_document_reaches_a_level_wise_joiner() {
    let label = "level-wise joiner";
    let mut nodes: Vec<SimNode> = ["ld0", "ld1"]
        .iter()
        .map(|name| SimNode::new_in_context((*name).to_owned(), context()))
        .collect();
    for node in &nodes {
        seed_document(node);
    }
    let mut oracles = Oracles::new();

    let mut created = BlockId((0, 0));
    let _bytes = edit::<Doc>(&nodes[0], |doc| {
        created = doc
            .insert_block(None, "paragraph", 0)
            .expect("insert_block should succeed");
    });
    let _old = oracles.insert(created, Oracle::new(replica_of(&nodes[0])));
    let passage = unique_chars(0, 0, PASSAGE_LEN);
    let _typed = type_into(&nodes[0], &mut oracles, created, 0, &passage);

    for _ in 0..SYNC_ROUNDS {
        if nodes[1].root_hash() == nodes[0].root_hash() {
            break;
        }
        let (left, right) = nodes.split_at_mut(1);
        execute_level_wise_sync(&mut right[0], &left[0])
            .await
            .expect("level-wise sync should succeed");
    }
    let refs: Vec<&SimNode> = nodes.iter().collect();
    assert_document_properties(label, &refs, &oracles, &[passage]);
}
