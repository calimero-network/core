//! Multi-replica `RichText` scenarios over the real sync paths: text and marks
//! travel as real `StorageDelta` bytes through delta delivery, HashComparison
//! or a level-wise session.
//!
//! The character layer keeps `text.rs`'s pure-`FugueTree` oracle, so passage
//! contiguity and per-character identity are checked against something
//! independent. The mark layer is checked by properties that do not re-derive
//! the boundary sweep - every written row reaches every replica, every replica
//! renders byte-identical spans, and those spans spell exactly the oracle's
//! text. Differentiating the sweep itself against a naive model is
//! `calimero-storage`'s `rich_text_fuzz`, and duplicating it here would only
//! duplicate its mistakes.

use std::collections::{BTreeSet, HashSet};

use calimero_storage::collections::{DefaultMarks, DeltaOp, Mark, MarkId, RichText, Span};
use calimero_storage::store::MainStorage;

use crate::sync_sim::node::SimNode;
use crate::sync_sim::protocol::execute_level_wise_sync;
use crate::sync_sim::runtime::SimRng;
use crate::sync_sim::scenarios::text::{
    assert_rows_tagged, changed, context, converge_group, delivery_pass, edit, land, pull, read,
    replica_of, seed_doc, unique_chars, Oracle, Pending, DELIVERY_PASSES, EDIT_ROUNDS, SYNC_ROUNDS,
};

type Rich = RichText<DefaultMarks, MainStorage>;

const FIELD: &str = "sim_rich_doc";
const PASSAGE_LEN: usize = 24; // marks make the read quadratic in the model, so keep it short
const KEYS: [&str; 3] = ["bold", "italic", "comment:a"]; // After, After and None expands
const SEED_LEN: usize = 12; // characters every replica starts from

/// The ids this replica holds for every mark row, which is what "the row
/// reached everyone" means without re-deriving what the row renders as.
fn mark_ids(node: &SimNode) -> BTreeSet<MarkId> {
    rich_marks(node).into_iter().map(|mark| mark.id).collect()
}

fn rich_marks(node: &SimNode) -> Vec<Mark> {
    read::<Rich, _>(node, |doc| doc.marks().expect("marks should be readable"))
}

fn spans_of(node: &SimNode) -> Vec<Span> {
    read::<Rich, _>(node, |doc| {
        doc.to_delta().expect("to_delta should be readable")
    })
}

fn text_of(node: &SimNode) -> String {
    read::<Rich, _>(node, |doc| doc.get_text().expect("text should be readable"))
}

fn len_of(node: &SimNode) -> usize {
    read::<Rich, _>(node, |doc| doc.len().expect("len should be readable"))
}

/// The standing assertions every rich scenario ends with.
fn assert_rich_properties(
    label: &str,
    nodes: &[&SimNode],
    expected: &str,
    passages: &[String],
    written: &BTreeSet<MarkId>,
) {
    let first = nodes.first().expect("at least one replica");
    let reference = spans_of(first);
    let root = first.root_hash();

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
        assert_eq!(
            text,
            expected,
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

        let spans = spans_of(node);
        assert_eq!(
            spans,
            reference,
            "{label}: {} renders different spans",
            node.id()
        );
        assert_eq!(
            spans
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>(),
            text,
            "{label}: {} spans do not spell its own document",
            node.id()
        );
        assert_eq!(
            mark_ids(node),
            *written,
            "{label}: {} is missing mark rows, or holds ones nobody wrote",
            node.id()
        );

        assert_eq!(
            assert_rows_tagged(label, node),
            rich_marks(node).len(),
            "{label}: {} stores a different number of mark rows than it reads back",
            node.id()
        );
        assert_eq!(
            node.root_hash(),
            root,
            "{label}: {} merkle root differs",
            node.id()
        );
    }

    assert!(
        spans_of(first)
            .iter()
            .any(|span| !span.attributes.is_empty()),
        "{label}: no span carries any formatting, so the mark assertions are vacuous"
    );
}

/// One random rich edit: type a run, delete a range, or format one.
fn random_rich_edit(
    node: &SimNode,
    oracle: &mut Oracle,
    index: usize,
    minted: &mut usize,
    rng: &mut SimRng,
) -> (Vec<u8>, Vec<MarkId>) {
    let len = oracle.len();
    let mut written = Vec::new();
    let bytes = match rng.gen_range_usize(4) {
        0 if len > 1 => {
            let start = rng.gen_range_usize(len - 1);
            let end = start + 1 + rng.gen_range_usize(len - start - 1);
            let key = KEYS[rng.gen_range_usize(KEYS.len())];
            let value = (rng.gen_range_usize(3) != 0).then_some("true");
            edit::<Rich>(node, |doc| {
                if let Some(id) = doc
                    .mark(start, end, key, value)
                    .expect("mark should succeed")
                {
                    written.push(id);
                }
            })
        }
        1 if len > 1 => {
            let start = rng.gen_range_usize(len - 1);
            let count = 1 + rng.gen_range_usize((len - start).min(3));
            let bytes = edit::<Rich>(node, |doc| {
                let _undo = doc
                    .apply_delta(&[DeltaOp::retain(start), DeltaOp::Delete { delete: count }])
                    .expect("delete should succeed");
            });
            oracle.delete_range(start, start + count);
            bytes
        }
        _ => {
            let run = 1 + rng.gen_range_usize(4);
            let text = unique_chars(index, *minted, run);
            *minted += run;
            let pos = rng.gen_range_usize(len + 1);
            let bytes = edit::<Rich>(node, |doc| {
                let _undo = doc
                    .apply_delta(&[DeltaOp::retain(pos), DeltaOp::insert(&text)])
                    .expect("insert should succeed");
            });
            oracle.insert_str(pos, &text);
            bytes
        }
    };
    (bytes, written)
}

/// Seed every replica from one authored passage, so they share a document to
/// disagree about rather than three disjoint ones.
fn seeded_mesh(names: &[String]) -> (Vec<SimNode>, Vec<Oracle>, Vec<usize>) {
    let nodes: Vec<SimNode> = names
        .iter()
        .map(|name| SimNode::new_in_context(name.clone(), context()))
        .collect();
    for node in &nodes {
        seed_doc(node, || Rich::new_with_field_name(FIELD));
    }
    let mut oracles: Vec<Oracle> = nodes.iter().map(|n| Oracle::new(replica_of(n))).collect();
    let mut minted = vec![0_usize; nodes.len()];

    let base = unique_chars(0, 0, SEED_LEN);
    minted[0] += SEED_LEN;
    let seeded = edit::<Rich>(&nodes[0], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::insert(&base)])
            .expect("seed insert should succeed");
    });
    oracles[0].insert_str(0, &base);
    for (index, node) in nodes.iter().enumerate().skip(1) {
        land(node, &seeded);
        let nodes_of_first = oracles[0].nodes();
        oracles[index].integrate_all(&nodes_of_first);
    }
    (nodes, oracles, minted)
}

#[tokio::test]
async fn rich_marks_converge_under_lossy_delta_delivery() {
    for seed in 0..2_u64 {
        let label = format!("rich lossy delivery (seed {seed})");
        let count = 3 + usize::try_from(seed % 2).expect("modulus fits usize");
        let mut rng = SimRng::new(0x_21_C4_00 + seed);

        let names: Vec<String> = (0..count).map(|i| format!("r{i}")).collect();
        let (nodes, mut oracles, mut minted) = seeded_mesh(&names);
        let mut applied: Vec<HashSet<(usize, usize)>> = vec![HashSet::new(); count];
        let mut deltas: Vec<Pending> = Vec::new();
        let mut queue: Vec<(usize, usize)> = Vec::new();
        let mut written: BTreeSet<MarkId> = BTreeSet::new();

        for round in 0..=EDIT_ROUNDS {
            // The final round types one passage per replica with nothing
            // deleting after it, so contiguity is not racing a delete.
            let last = round == EDIT_ROUNDS;
            for author in 0..count {
                let before = oracles[author].snapshot();
                let bytes = if last {
                    let passage = unique_chars(author, minted[author], PASSAGE_LEN);
                    minted[author] += PASSAGE_LEN;
                    let pos = rng.gen_range_usize(oracles[author].len() + 1);
                    let bytes = edit::<Rich>(&nodes[author], |doc| {
                        let _undo = doc
                            .apply_delta(&[DeltaOp::retain(pos), DeltaOp::insert(&passage)])
                            .expect("passage insert");
                    });
                    oracles[author].insert_str(pos, &passage);
                    bytes
                } else {
                    let (bytes, ids) = random_rich_edit(
                        &nodes[author],
                        &mut oracles[author],
                        author,
                        &mut minted[author],
                        &mut rng,
                    );
                    written.extend(ids);
                    bytes
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
                drain_once(
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
            drain_once(
                &nodes,
                &mut oracles,
                &deltas,
                &mut queue,
                &mut applied,
                &mut rng,
            );
        }
        assert!(queue.is_empty(), "{label}: delivery queue never drained");
        assert!(
            !written.is_empty(),
            "{label}: no mark was ever written, so the scenario tests only text"
        );

        let passages: Vec<String> = (0..count)
            .map(|author| unique_chars(author, minted[author] - PASSAGE_LEN, PASSAGE_LEN))
            .collect();
        let refs: Vec<&SimNode> = nodes.iter().collect();
        assert_rich_properties(&label, &refs, &oracles[0].text(), &passages, &written);
    }
}

fn drain_once(
    nodes: &[SimNode],
    oracles: &mut [Oracle],
    deltas: &[Pending],
    queue: &mut Vec<(usize, usize)>,
    applied: &mut [HashSet<(usize, usize)>],
    rng: &mut SimRng,
) {
    delivery_pass(
        nodes,
        &mut |to, delta| oracles[to].integrate_all(&delta.nodes),
        deltas,
        queue,
        applied,
        rng,
    );
}

/// A mark that arrives before the text it names must be inert rather than an
/// error, and must become correct the moment the text lands.
#[tokio::test]
async fn rich_mark_arriving_before_its_text_is_inert_then_correct() {
    let label = "mark before its text";
    let names: Vec<String> = ["mb0", "mb1"].iter().map(|n| (*n).to_owned()).collect();
    let (nodes, mut oracles, mut minted) = seeded_mesh(&names);

    let typed_text = unique_chars(0, minted[0], 6);
    minted[0] += 6;
    let typed = edit::<Rich>(&nodes[0], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(SEED_LEN), DeltaOp::insert(&typed_text)])
            .expect("typing should succeed");
    });
    oracles[0].insert_str(SEED_LEN, &typed_text);

    let mut written = BTreeSet::new();
    let formatted = edit::<Rich>(&nodes[0], |doc| {
        let id = doc
            .mark(SEED_LEN, SEED_LEN + 6, "bold", Some("true"))
            .expect("mark should succeed")
            .expect("the mark must not be suppressed");
        written.insert(id);
    });

    // Out of causal order on purpose: the mark first.
    land(&nodes[1], &formatted);
    let inert = spans_of(&nodes[1]);
    assert_eq!(
        inert.len(),
        1,
        "{label}: an unplaceable mark must render nothing, got {inert:?}"
    );
    assert!(
        inert[0].attributes.is_empty(),
        "{label}: an unplaceable mark formatted something"
    );
    assert_eq!(
        mark_ids(&nodes[1]),
        written,
        "{label}: the row must be retained even though it resolves nowhere"
    );

    land(&nodes[1], &typed);
    let nodes_of_first = oracles[0].nodes();
    oracles[1].integrate_all(&nodes_of_first);
    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1]],
        &oracles[0].text(),
        &[typed_text],
        &written,
    );
}

/// Landing the same delta three times, and an older one after a newer one.
#[tokio::test]
async fn rich_duplicate_and_stale_mark_delivery_is_idempotent() {
    let label = "duplicate and stale delivery";
    let names: Vec<String> = ["dz0", "dz1"].iter().map(|n| (*n).to_owned()).collect();
    let (nodes, oracles, _minted) = seeded_mesh(&names);

    let mut written = BTreeSet::new();
    let first = edit::<Rich>(&nodes[0], |doc| {
        written.insert(
            doc.mark(0, 6, "bold", Some("true"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
    });
    let second = edit::<Rich>(&nodes[0], |doc| {
        written.insert(
            doc.mark(3, SEED_LEN, "italic", Some("true"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
    });

    for delta in [&second, &first, &second, &first, &first] {
        land(&nodes[1], delta);
    }
    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1]],
        &oracles[0].text(),
        &[],
        &written,
    );
}

/// One side only formats, the other only types at the same spot. After the heal
/// the typed text must be formatted exactly as the expand rules say.
#[tokio::test]
async fn rich_partition_format_versus_type() {
    let label = "partition: format versus type";
    let names: Vec<String> = ["pf0", "pf1"].iter().map(|n| (*n).to_owned()).collect();
    let (mut nodes, mut oracles, mut minted) = seeded_mesh(&names);

    let mut written = BTreeSet::new();
    let _formatted = edit::<Rich>(&nodes[0], |doc| {
        written.insert(
            doc.mark(0, 6, "bold", Some("true"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
        written.insert(
            doc.mark(0, 6, "comment:a", Some("look"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
    });

    let typed = unique_chars(1, minted[1], 4);
    minted[1] += 4;
    let _edit = edit::<Rich>(&nodes[1], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(6), DeltaOp::insert(&typed)])
            .expect("typing should succeed");
    });
    oracles[1].insert_str(6, &typed);

    assert_ne!(
        nodes[0].root_hash(),
        nodes[1].root_hash(),
        "{label}: the two sides must differ before the heal"
    );
    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "{label}: the mesh did not converge after the heal"
    );
    let nodes_of_second = oracles[1].nodes();
    oracles[0].integrate_all(&nodes_of_second);

    // Bold's end anchor grew over the typed run; the comment's did not.
    let spans = spans_of(&nodes[0]);
    let typed_span = spans
        .iter()
        .find(|span| span.text.contains(typed.as_str()))
        .unwrap_or_else(|| panic!("{label}: the typed run vanished: {spans:?}"));
    assert_eq!(
        typed_span.attributes.get("bold").map(String::as_str),
        Some("true"),
        "{label}: bold must grow over text typed at its end"
    );
    assert_eq!(
        typed_span.attributes.get("comment:a"),
        None,
        "{label}: a comment must not grow over text typed at its end"
    );

    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1]],
        &oracles[0].text(),
        &[typed],
        &written,
    );
}

/// One side formats a word while the other deletes it; the heal is
/// HashComparison only, and the replicas must end byte-identical.
#[tokio::test]
async fn rich_partition_format_versus_delete() {
    let label = "partition: format versus delete";
    let names: Vec<String> = ["fd0", "fd1"].iter().map(|n| (*n).to_owned()).collect();
    let (mut nodes, oracles, _minted) = seeded_mesh(&names);

    let mut written = BTreeSet::new();
    let _formatted = edit::<Rich>(&nodes[0], |doc| {
        written.insert(
            doc.mark(4, 7, "bold", Some("true"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
        // Uncontested, so the read-back checks have formatting to compare.
        written.insert(
            doc.mark(0, 3, "italic", Some("true"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
    });
    let _deleted = edit::<Rich>(&nodes[1], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(4), DeltaOp::Delete { delete: 3 }])
            .expect("delete should succeed");
    });

    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "{label}: the mesh did not converge after the heal"
    );
    let mut expected: Vec<char> = oracles[0].text().chars().collect();
    let _cut: Vec<char> = expected.drain(4..7).collect();
    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1]],
        &expected.into_iter().collect::<String>(),
        &[],
        &written,
    );
}

/// The heal the e2e runs: three nodes, one cut off while the other two keep
/// exchanging deltas, every contested case at once, then HashComparison.
#[tokio::test]
async fn rich_partition_three_nodes_every_contested_case() {
    let label = "partition: three nodes, every contested case";
    let names: Vec<String> = ["tn0", "tn1", "tn2"]
        .iter()
        .map(|n| (*n).to_owned())
        .collect();
    let (mut nodes, oracles, _minted) = seeded_mesh(&names);

    let mut written = BTreeSet::new();
    let mut mark = |node: &SimNode, start, end, key: &str, value: Option<&str>| {
        edit::<Rich>(node, |doc| {
            if let Some(id) = doc
                .mark(start, end, key, value)
                .expect("mark should succeed")
            {
                written.insert(id);
            }
        })
    };
    // The cut-off side: its deltas reach nobody until the heal.
    let _ = mark(&nodes[0], 0, SEED_LEN, "bold", Some("true"));
    let _ = mark(&nodes[0], 4, SEED_LEN, "bold", None);
    let _ = mark(&nodes[0], 4, 7, "bold", Some("true"));
    let _ = mark(&nodes[0], 4, 7, "comment:a", Some("first"));
    // The connected side: every delta lands on its peer at once.
    let peer = [
        mark(&nodes[1], 8, SEED_LEN, "bold", Some("true")),
        edit::<Rich>(&nodes[1], |doc| {
            let _undo = doc
                .apply_delta(&[DeltaOp::retain(4), DeltaOp::Delete { delete: 3 }])
                .expect("delete should succeed");
        }),
    ];
    for delta in &peer {
        land(&nodes[2], delta);
    }

    assert!(
        converge_group(&mut nodes, &[0, 1, 2]).await,
        "{label}: the mesh did not converge after the heal"
    );
    let mut expected: Vec<char> = oracles[0].text().chars().collect();
    let _cut: Vec<char> = expected.drain(4..7).collect();
    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1], &nodes[2]],
        &expected.into_iter().collect::<String>(),
        &[],
        &written,
    );
}

/// A joiner that catches up over HashComparison must end byte-identical.
#[tokio::test]
async fn rich_marks_reach_a_hash_comparison_joiner() {
    let label = "hash-comparison joiner";
    let names: Vec<String> = ["hc0", "hc1"].iter().map(|n| (*n).to_owned()).collect();
    let mut nodes: Vec<SimNode> = names
        .iter()
        .map(|name| SimNode::new_in_context(name.clone(), context()))
        .collect();
    for node in &nodes {
        seed_doc(node, || Rich::new_with_field_name(FIELD));
    }
    let mut oracle = Oracle::new(replica_of(&nodes[0]));

    let passage = unique_chars(0, 0, PASSAGE_LEN);
    let _seeded = edit::<Rich>(&nodes[0], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::insert(&passage)])
            .expect("seed insert should succeed");
    });
    oracle.insert_str(0, &passage);

    let mut written = BTreeSet::new();
    let _formatted = edit::<Rich>(&nodes[0], |doc| {
        for (start, end, key) in [(0, 6, "bold"), (4, 12, "italic"), (8, 16, "comment:a")] {
            written.insert(
                doc.mark(start, end, key, Some("true"))
                    .expect("mark should succeed")
                    .expect("not suppressed"),
            );
        }
    });

    for _ in 0..SYNC_ROUNDS {
        pull(&mut nodes, 1, 0).await;
    }
    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1]],
        &oracle.text(),
        &[passage],
        &written,
    );
}

/// The level-wise path hits the same wall the nested text scenario documents:
/// a collection container has no room for its own row on that wire, so a joiner
/// rebuilds it from its children and its `own_hash` never settles.
#[tokio::test]
#[ignore = "level-wise cannot carry a collection container's own row without a LevelNode wire field"]
async fn rich_marks_reach_a_level_wise_joiner() {
    let label = "level-wise joiner";
    let names: Vec<String> = ["lw0", "lw1"].iter().map(|n| (*n).to_owned()).collect();
    let mut nodes: Vec<SimNode> = names
        .iter()
        .map(|name| SimNode::new_in_context(name.clone(), context()))
        .collect();
    for node in &nodes {
        seed_doc(node, || Rich::new_with_field_name(FIELD));
    }
    let mut oracle = Oracle::new(replica_of(&nodes[0]));

    let passage = unique_chars(0, 0, PASSAGE_LEN);
    let _seeded = edit::<Rich>(&nodes[0], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::insert(&passage)])
            .expect("seed insert should succeed");
    });
    oracle.insert_str(0, &passage);

    let mut written = BTreeSet::new();
    let _formatted = edit::<Rich>(&nodes[0], |doc| {
        written.insert(
            doc.mark(0, 6, "bold", Some("true"))
                .expect("mark should succeed")
                .expect("not suppressed"),
        );
    });

    for _ in 0..SYNC_ROUNDS {
        if nodes[1].root_hash() == nodes[0].root_hash() {
            break;
        }
        let (left, right) = nodes.split_at_mut(1);
        execute_level_wise_sync(&mut right[0], &left[0])
            .await
            .expect("level-wise sync should succeed");
    }
    assert_rich_properties(
        label,
        &[&nodes[0], &nodes[1]],
        &oracle.text(),
        &[passage],
        &written,
    );
}

/// An anchor a mark holds must name the same character on every replica, which
/// is the property every span in the document rests on.
#[tokio::test]
async fn rich_mark_anchors_name_the_same_character_on_every_replica() {
    let label = "anchor identity";
    let names: Vec<String> = ["an0", "an1"].iter().map(|n| (*n).to_owned()).collect();
    let (mut nodes, mut oracles, mut minted) = seeded_mesh(&names);

    let _formatted = edit::<Rich>(&nodes[0], |doc| {
        let _id = doc
            .mark(2, 8, "bold", Some("true"))
            .expect("mark should succeed");
    });
    let held = rich_marks(&nodes[0]);
    assert_eq!(held.len(), 1);

    let typed = unique_chars(1, minted[1], 3);
    minted[1] += 3;
    let _edit = edit::<Rich>(&nodes[1], |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(0), DeltaOp::insert(&typed)])
            .expect("typing should succeed");
    });
    oracles[1].insert_str(0, &typed);

    assert!(
        converge_group(&mut nodes, &[0, 1]).await,
        "{label}: the mesh did not converge"
    );
    let nodes_of_second = oracles[1].nodes();
    oracles[0].integrate_all(&nodes_of_second);

    // Three characters landed before the range, so both anchors moved by three.
    let anchors = [held[0].start, held[0].end];
    let resolved: Vec<Option<usize>> = read::<Rich, _>(&nodes[0], |doc| {
        doc.resolve_many(&anchors).expect("anchors should resolve")
    });
    assert_eq!(
        resolved,
        vec![Some(5), Some(11)],
        "{label}: the anchors did not follow their characters"
    );
    assert_eq!(
        resolved,
        anchors
            .iter()
            .map(|anchor| oracles[0].resolve(anchor))
            .collect::<Vec<_>>(),
        "{label}: the collection and the oracle place the anchors differently"
    );
}
