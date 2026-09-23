//! `RichText` unit behaviour: every rule, every error, every boundary, plus the
//! merge laws and the row-count budget.
//!
//! Row counts are read off the delta a write emits rather than off `marks()`,
//! so a write that is immediately overwritten still shows up.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use calimero_sdk::app::Mergeable;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::fugue_text::{Anchor, Bias};
use calimero_storage::collections::{
    Attrs, DefaultMarks, DeltaOp, DeltaUndo, Mark, MarkId, RichText, Span, UndoStep, UnorderedMap,
};
use calimero_storage::env;
use calimero_storage::store::MainStorage;

mod fugue_harness;
mod rich_text_model;

use fugue_harness::{
    device, edit, entry_bytes, fork, genesis, land, permutations, read_with, root_hash,
    written_ids, Store,
};
use rich_text_model::{expect, readable, BOLD, NONE};

const ALICE: u8 = 1;
const BOB: u8 = 2;
const SENTENCE: &str = "The fox jumped.";
const TOGGLES: usize = 1_000; // enough to make unbounded mark growth visible
const WIDE_RANGE: usize = 5_000; // one mark over a large range is still one row

type Doc = RichText<DefaultMarks, MainStorage>;

// ── harness ─────────────────────────────────────────────────────────────

fn seeded(text: &str) -> Store {
    genesis(
        || Doc::new_with_field_name("doc"),
        |doc| {
            if !text.is_empty() {
                let _undo = doc.apply_delta(&[DeltaOp::insert(text)]).unwrap();
            }
        },
    )
}

fn attrs(pairs: &[(&str, Option<&str>)]) -> Attrs {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.map(str::to_owned)))
        .collect()
}

fn spans_of(store: &Store) -> Vec<Span> {
    read_with::<Doc, _>(store, device(ALICE), |doc| doc.to_delta().unwrap())
}

fn text_of(store: &Store) -> String {
    read_with::<Doc, _>(store, device(ALICE), |doc| doc.get_text().unwrap())
}

fn marks_of(store: &Store) -> Vec<Mark> {
    read_with::<Doc, _>(store, device(ALICE), |doc| doc.marks().unwrap())
}

/// The rows a transaction writes when it changes nothing at all. Every "wrote
/// no row" assertion is read against this control: taking the document mutably
/// dirties the state container whether or not a collection changed, so an empty
/// id set is not what "no row" looks like.
fn no_op_rows(store: &Store) -> BTreeSet<Id> {
    written_ids(&edit::<Doc>(store, device(ALICE), |doc| {
        let _undo = doc.apply_delta(&[DeltaOp::retain(0)]).unwrap();
    }))
    .into_iter()
    .collect()
}

fn assert_wrote_no_row(label: &str, control: &BTreeSet<Id>, delta: &[u8]) {
    let ids: BTreeSet<Id> = written_ids(delta).into_iter().collect();
    assert_eq!(
        &ids, control,
        "{label}: the write touched rows a no-op transaction does not"
    );
}

/// Type `text` at visible position `pos`, carrying `attributes` verbatim.
fn type_at(
    store: &Store,
    writer: u8,
    pos: usize,
    text: &str,
    attributes: Option<Attrs>,
) -> Vec<u8> {
    edit::<Doc>(store, device(writer), |doc| {
        let _undo = doc
            .apply_delta(&[
                DeltaOp::retain(pos),
                DeltaOp::Insert {
                    insert: text.to_owned(),
                    attributes,
                },
            ])
            .unwrap();
    })
}

// ── the rules ───────────────────────────────────────────────────────────

#[test]
fn mark__renders_the_range_it_names() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(4, 7, "bold", Some("true")).unwrap();
    });
    expect(
        &spans_of(&store),
        &[("The ", NONE), ("fox", BOLD), (" jumped.", NONE)],
    );
}

#[test]
fn mark__re_asserting_existing_formatting_writes_no_row() {
    let store = seeded(SENTENCE);
    let _first = edit::<Doc>(&store, device(ALICE), |doc| {
        assert!(doc.mark(4, 7, "bold", Some("true")).unwrap().is_some());
    });
    let control = no_op_rows(&store);
    let again = edit::<Doc>(&store, device(ALICE), |doc| {
        assert_eq!(
            doc.mark(4, 7, "bold", Some("true")).unwrap(),
            None,
            "an already-bold range must suppress the write"
        );
    });
    assert_wrote_no_row("re-asserted bold", &control, &again);
    assert_eq!(marks_of(&store).len(), 1);
}

#[test]
fn mark__a_subrange_of_an_existing_mark_is_also_redundant() {
    let store = seeded(SENTENCE);
    let _first = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, 15, "bold", Some("true")).unwrap();
    });
    let control = no_op_rows(&store);
    let again = edit::<Doc>(&store, device(ALICE), |doc| {
        assert_eq!(doc.mark(4, 7, "bold", Some("true")).unwrap(), None);
    });
    assert_wrote_no_row("bolding a subrange of a bold range", &control, &again);
}

#[test]
fn unmark__of_a_key_that_is_not_set_writes_no_row() {
    let store = seeded(SENTENCE);
    let control = no_op_rows(&store);
    let delta = edit::<Doc>(&store, device(ALICE), |doc| {
        assert_eq!(doc.unmark(0, 15, "bold").unwrap(), None);
    });
    assert_wrote_no_row("unmarking an unset key", &control, &delta);
    assert!(marks_of(&store).is_empty());
}

#[test]
fn mark__rejects_a_key_the_schema_does_not_declare() {
    let store = seeded(SENTENCE);
    let control = no_op_rows(&store);
    let delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let message = doc.unmark(0, 15, "nonsense").unwrap_err().to_string();
        assert!(message.contains("unknown mark key 'nonsense'"), "{message}");
    });
    assert_wrote_no_row("an undeclared key", &control, &delta);
    assert!(marks_of(&store).is_empty());
}

#[test]
fn mark__rejects_an_inverted_range_and_a_range_past_the_end() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        assert!(doc
            .mark(7, 4, "bold", Some("true"))
            .unwrap_err()
            .to_string()
            .contains("start must be <= end"));
        assert!(doc
            .mark(0, 99, "bold", Some("true"))
            .unwrap_err()
            .to_string()
            .contains("out of bounds"));
    });
    assert!(marks_of(&store).is_empty());
}

#[test]
fn mark__an_empty_range_writes_nothing() {
    let store = seeded(SENTENCE);
    let control = no_op_rows(&store);
    let delta = edit::<Doc>(&store, device(ALICE), |doc| {
        assert_eq!(doc.mark(3, 3, "bold", Some("true")).unwrap(), None);
    });
    assert_wrote_no_row("an empty range", &control, &delta);
    assert!(marks_of(&store).is_empty());
}

#[test]
fn mark__one_row_covers_a_range_of_any_size() {
    let store = seeded(&"a".repeat(WIDE_RANGE));
    let control = no_op_rows(&store);
    let delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, WIDE_RANGE, "bold", Some("true")).unwrap();
    });
    assert_eq!(marks_of(&store).len(), 1, "one mark must be one row");
    let extra: BTreeSet<Id> = written_ids(&delta)
        .into_iter()
        .filter(|id| !control.contains(id))
        .collect();
    assert_eq!(
        extra.len(),
        1,
        "a mark over {WIDE_RANGE} characters wrote {} rows",
        extra.len()
    );
}

#[test]
fn to_delta__skips_a_mark_that_names_a_character_this_replica_lacks() {
    let store = seeded(SENTENCE);
    let absent = Anchor::Char {
        id: (0xDEAD_BEEF, 3),
        bias: Bias::Before,
    };
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc
            .mark_at(absent, Anchor::End, "bold", Some("true"))
            .unwrap();
    });
    expect(&spans_of(&store), &[(SENTENCE, NONE)]);
    assert_eq!(
        marks_of(&store).len(),
        1,
        "the row must be retained, or a replica that has the text would diverge"
    );
}

#[test]
fn to_delta__skips_a_mark_whose_start_lands_at_or_after_its_end() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let start = doc.anchor_at(10, Bias::Before).unwrap();
        let end = doc.anchor_at(2, Bias::Before).unwrap();
        let _id = doc.mark_at(start, end, "bold", Some("true")).unwrap();
    });
    expect(&spans_of(&store), &[(SENTENCE, NONE)]);
    assert_eq!(marks_of(&store).len(), 1);
}

#[test]
#[should_panic(expected = "mark_with_replica")]
fn mark__panics_during_a_migration() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        env::with_merge_mode(|| {
            let _ignored = doc.mark(0, 4, "bold", Some("true"));
        });
    });
}

#[test]
#[should_panic(expected = "mark_with_replica")]
fn apply_delta__panics_during_a_migration() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        env::with_merge_mode(|| {
            let _ignored = doc.apply_delta(&[DeltaOp::insert("x")]);
        });
    });
}

#[test]
fn mark_with_replica__is_allowed_during_a_migration() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        env::with_merge_mode(|| {
            let id = doc
                .mark_with_replica(0, 4, "bold", Some("true"), 7)
                .unwrap()
                .unwrap();
            assert_eq!(
                id,
                MarkId {
                    lamport: 1,
                    replica: 7
                }
            );
        });
    });
    expect(&spans_of(&store), &[("The ", BOLD), ("fox jumped.", NONE)]);
}

#[test]
fn apply_delta__an_insert_past_the_end_stores_nothing_anywhere() {
    let store = seeded("abc");
    let _first = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, 3, "bold", Some("true")).unwrap();
    });
    let before_marks = marks_of(&store);
    let control = no_op_rows(&store);

    let delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let result = doc.apply_delta(&[
            DeltaOp::Retain {
                retain: 3,
                attributes: Some(attrs(&[("italic", Some("true"))])),
            },
            DeltaOp::retain(20),
            DeltaOp::insert("Y"),
        ]);
        assert!(result.is_err(), "an insert past the end must fail");
    });
    assert_wrote_no_row("a rejected delta", &control, &delta);
    assert_eq!(text_of(&store), "abc");
    assert_eq!(marks_of(&store), before_marks, "the italic run was written");
}

#[test]
fn apply_delta__a_delete_past_the_end_clamps_like_the_text_layer_does() {
    let store = seeded("abc");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(1), DeltaOp::Delete { delete: 99 }])
            .unwrap();
    });
    assert_eq!(text_of(&store), "a");
}

// ── boundaries ──────────────────────────────────────────────────────────

/// `(span text, whether that span carries `key`)` for the character at `at`.
fn attr_at(store: &Store, at: usize, key: &str) -> Option<String> {
    let mut seen = 0;
    for span in spans_of(store) {
        let len = span.text.chars().count();
        if at < seen + len {
            return span.attributes.get(key).cloned();
        }
        seen += len;
    }
    None
}

fn bolded(range: (usize, usize)) -> Store {
    let store = seeded("abcde");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(range.0, range.1, "bold", Some("true")).unwrap();
    });
    store
}

fn linked(range: (usize, usize)) -> Store {
    let store = seeded("abcde");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(range.0, range.1, "link", Some("u")).unwrap();
    });
    store
}

#[test]
fn boundary__bold_grows_at_its_end_and_not_at_its_start() {
    let store = bolded((1, 4));
    let _delta = type_at(&store, ALICE, 4, "X", None);
    assert_eq!(text_of(&store), "abcdXe");
    assert_eq!(attr_at(&store, 4, "bold").as_deref(), Some("true"));

    let store = bolded((1, 4));
    let _delta = type_at(&store, ALICE, 1, "X", None);
    assert_eq!(text_of(&store), "aXbcde");
    assert_eq!(attr_at(&store, 1, "bold"), None);
}

#[test]
fn boundary__a_link_grows_at_neither_edge() {
    for (pos, expected) in [(4_usize, "abcdXe"), (1, "aXbcde")] {
        let store = linked((1, 4));
        let _delta = type_at(&store, ALICE, pos, "X", None);
        assert_eq!(text_of(&store), expected);
        assert_eq!(attr_at(&store, pos, "link"), None, "at {pos}");
    }
}

/// Peritext's tombstone rule, both sides of it. Bold's end anchor is not an
/// `After` anchor, so the typed character stays inside the range; the link's is,
/// so the scan places the character past the tombstone and out of the link.
#[test]
fn boundary__typing_at_a_deleted_edge_keeps_bold_and_drops_the_link() {
    let store = bolded((1, 4));
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(3), DeltaOp::Delete { delete: 1 }])
            .unwrap();
    });
    let _delta = type_at(&store, ALICE, 3, "X", None);
    assert_eq!(text_of(&store), "abcXe");
    assert_eq!(attr_at(&store, 3, "bold").as_deref(), Some("true"));

    let store = linked((1, 4));
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(3), DeltaOp::Delete { delete: 1 }])
            .unwrap();
    });
    let _delta = type_at(&store, ALICE, 3, "X", None);
    assert_eq!(text_of(&store), "abcXe");
    assert_eq!(attr_at(&store, 3, "link"), None);
}

#[test]
fn insert__with_an_empty_attribute_set_strips_what_it_inherited() {
    let store = seeded("abc");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, 3, "bold", Some("true")).unwrap();
    });
    let delta = type_at(&store, ALICE, 3, "d", Some(Attrs::new()));
    assert_eq!(attr_at(&store, 3, "bold"), None, "d must not be bold");
    assert_eq!(
        marks_of(&store).len(),
        2,
        "exactly one correction row: {:?}",
        marks_of(&store)
    );
    assert!(!written_ids(&delta).is_empty());

    // The correction's own end anchor grows, because removing an `After` expand
    // inverts to `After`. So the next character typed there is not bold either.
    let _delta = type_at(&store, ALICE, 4, "e", None);
    assert_eq!(attr_at(&store, 4, "bold"), None);
    assert_eq!(marks_of(&store).len(), 2, "no second correction was needed");
}

/// Bold's expand is `After`, which inverts to itself, so bold cannot tell an
/// inverted removal from a non-inverted one. A link's `None` inverts to `Both`,
/// which is what makes typing at either edge of an un-linked run stay un-linked.
#[test]
fn unmark__inverts_expand_so_both_edges_of_the_removal_grow() {
    for (typed_at, label) in [(1_usize, "the start"), (2, "the end")] {
        let store = seeded("abc");
        let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
            let _id = doc.mark(0, 3, "link", Some("u")).unwrap();
            let _id = doc.unmark(1, 2, "link").unwrap();
        });
        assert_eq!(attr_at(&store, 1, "link"), None, "b must be un-linked");

        let _delta = type_at(&store, ALICE, typed_at, "X", None);
        assert_eq!(
            attr_at(&store, typed_at, "link"),
            None,
            "the removal did not grow at {label}: {:?}",
            spans_of(&store)
        );
    }
}

#[test]
fn insert__with_no_attributes_at_a_non_growing_edge_writes_no_correction() {
    let store = seeded("abc");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, 3, "link", Some("u")).unwrap();
    });
    let _delta = type_at(&store, ALICE, 3, "d", None);
    assert_eq!(attr_at(&store, 3, "link"), None);
    assert_eq!(
        marks_of(&store).len(),
        1,
        "the link was never inherited, so nothing needed correcting"
    );
}

#[test]
fn insert__with_no_attributes_inherits_at_a_growing_edge() {
    let store = seeded("abc");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, 3, "bold", Some("true")).unwrap();
    });
    let _delta = type_at(&store, ALICE, 3, "d", None);
    assert_eq!(attr_at(&store, 3, "bold").as_deref(), Some("true"));
    assert_eq!(marks_of(&store).len(), 1);
}

#[test]
fn insert__an_explicit_value_beats_what_it_inherited() {
    let store = seeded("abc");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(0, 3, "bold", Some("true")).unwrap();
    });
    let _delta = type_at(
        &store,
        ALICE,
        3,
        "d",
        Some(attrs(&[("bold", Some("true")), ("italic", Some("true"))])),
    );
    assert_eq!(attr_at(&store, 3, "italic").as_deref(), Some("true"));
    assert_eq!(attr_at(&store, 3, "bold").as_deref(), Some("true"));
    assert_eq!(
        marks_of(&store).len(),
        2,
        "bold was already inherited, so only italic needed a row"
    );
}

#[test]
fn astral_characters_count_as_one_position_each() {
    // Two astral characters and one BMP character: bytes, nodes and positions
    // all differ, so a byte-indexing bug cannot hide.
    let store = seeded("\u{1F600}\u{1F601}\u{00E9}");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(1, 3, "bold", Some("true")).unwrap();
    });
    expect(
        &spans_of(&store),
        &[("\u{1F600}", NONE), ("\u{1F601}\u{00E9}", BOLD)],
    );
}

// ── undo ────────────────────────────────────────────────────────────────

#[test]
fn undo__removes_the_characters_it_minted_and_leaves_a_peer_typing_inside() {
    let base = seeded("ab");
    let (left, right) = (fork(&base), fork(&base));

    let mut token = DeltaUndo(Vec::new());
    let typed = edit::<Doc>(&left, device(ALICE), |doc| {
        token = doc
            .apply_delta(&[DeltaOp::retain(1), DeltaOp::insert("XYZ")])
            .unwrap();
    });
    land(&right, device(BOB), &typed);
    let peer = edit::<Doc>(&right, device(BOB), |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(3), DeltaOp::insert("q")])
            .unwrap();
    });
    land(&left, device(ALICE), &peer);
    assert_eq!(text_of(&left), "aXYqZb");

    let _delta = edit::<Doc>(&left, device(ALICE), |doc| {
        let _redo = doc.apply_undo(&token).unwrap();
    });
    assert_eq!(text_of(&left), "aqb", "the peer's character must survive");
}

#[test]
fn undo__of_a_delete_restores_the_text_and_its_formatting() {
    let store = seeded("hello world");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _id = doc.mark(6, 11, "bold", Some("true")).unwrap();
    });
    expect(&spans_of(&store), &[("hello ", NONE), ("world", BOLD)]);

    let mut token = DeltaUndo(Vec::new());
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        token = doc
            .apply_delta(&[DeltaOp::retain(6), DeltaOp::Delete { delete: 5 }])
            .unwrap();
    });
    assert_eq!(text_of(&store), "hello ");

    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _redo = doc.apply_undo(&token).unwrap();
    });
    assert_eq!(text_of(&store), "hello world");
    assert_eq!(
        attr_at(&store, 6, "bold").as_deref(),
        Some("true"),
        "undo must restore the formatting the delete took: {:?}",
        spans_of(&store)
    );
}

#[test]
fn undo__of_an_undo_reproduces_the_original_change() {
    let store = seeded("hello world");
    let before = readable(&spans_of(&store));

    let mut token = DeltaUndo(Vec::new());
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        token = doc
            .apply_delta(&[
                DeltaOp::Retain {
                    retain: 5,
                    attributes: Some(attrs(&[("bold", Some("true"))])),
                },
                DeltaOp::Delete { delete: 6 },
                DeltaOp::insert("!"),
            ])
            .unwrap();
    });
    let after = readable(&spans_of(&store));
    assert_ne!(after, before);

    let mut redo = DeltaUndo(Vec::new());
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        redo = doc.apply_undo(&token).unwrap();
    });
    assert_eq!(readable(&spans_of(&store)), before, "undo did not unwind");

    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        let _again = doc.apply_undo(&redo).unwrap();
    });
    assert_eq!(readable(&spans_of(&store)), after, "redo did not replay");
}

#[test]
fn apply_delta__a_change_that_changes_nothing_has_an_empty_undo() {
    let store = seeded("abc");
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        assert_eq!(
            doc.apply_delta(&[DeltaOp::retain(3)]).unwrap(),
            DeltaUndo(Vec::new())
        );
        assert_eq!(doc.apply_delta(&[]).unwrap(), DeltaUndo(Vec::new()));
    });
    assert!(marks_of(&store).is_empty());
}

// ── merge laws ──────────────────────────────────────────────────────────

/// The same set of mark writes, minted once by independent authors, must land
/// in ANY delivery order to identical spans, identical stored bytes and an
/// identical Merkle root. Not just the rendered output: the bytes.
#[test]
fn merge__every_delivery_order_of_a_mark_set_converges_byte_for_byte() {
    type Step = (usize, usize, &'static str, Option<&'static str>);
    const STEPS: [Step; 5] = [
        (0, 7, "bold", Some("true")),
        (4, 15, "italic", Some("true")),
        (2, 9, "bold", None),
        (0, 15, "comment:a", Some("look")),
        (8, 15, "link", Some("u")),
    ];

    // One author per step, each in isolation, so their lamports genuinely race.
    let base = seeded(SENTENCE);
    let deltas: Vec<Vec<u8>> = STEPS
        .iter()
        .enumerate()
        .map(|(slot, (start, end, key, value))| {
            let store = fork(&base);
            edit::<Doc>(&store, device(u8::try_from(slot).unwrap() + 1), |doc| {
                let _id = doc.mark(*start, *end, key, *value).unwrap();
            })
        })
        .collect();

    let mut reference: Option<Converged> = None;
    for order in permutations(STEPS.len()) {
        let store = fork(&base);
        for index in &order {
            land(&store, device(ALICE), &deltas[*index]);
        }
        let outcome = Converged {
            spans: readable(&spans_of(&store)),
            rows: borsh::to_vec(&marks_of(&store)).unwrap(),
            root: root_hash(&store, ALICE),
        };
        match &reference {
            None => reference = Some(outcome),
            Some(want) => {
                assert_eq!(
                    outcome.spans, want.spans,
                    "order {order:?} rendered differently"
                );
                assert_eq!(
                    outcome.rows, want.rows,
                    "order {order:?} stored different bytes"
                );
                assert_eq!(
                    outcome.root, want.root,
                    "order {order:?} has a different root"
                );
            }
        }
    }
    let converged = reference.expect("at least one delivery order ran");
    assert!(
        converged.spans.len() > 1,
        "the fixture produced a single flat span"
    );
    assert!(!converged.rows.is_empty());
}

/// What every delivery order has to agree on.
struct Converged {
    spans: Vec<(String, Vec<(String, String)>)>,
    rows: Vec<u8>,
    root: Option<[u8; 32]>,
}

/// Landing the same delta three times, and an older one after a newer one, must
/// leave the rows exactly as one delivery did.
#[test]
fn merge__duplicate_and_stale_delivery_is_idempotent() {
    let base = seeded(SENTENCE);
    let (left, right) = (fork(&base), fork(&base));

    let first = edit::<Doc>(&left, device(ALICE), |doc| {
        let _id = doc.mark(0, 7, "bold", Some("true")).unwrap();
    });
    let second = edit::<Doc>(&left, device(ALICE), |doc| {
        let _id = doc.mark(4, 15, "italic", Some("true")).unwrap();
    });

    // Newest first, then the older one, then both again.
    for delta in [&second, &first, &second, &first, &first] {
        land(&right, device(BOB), delta);
    }
    let ids: BTreeSet<Id> = written_ids(&first)
        .into_iter()
        .chain(written_ids(&second))
        .collect();
    assert_eq!(entry_bytes(&left, &ids), entry_bytes(&right, &ids));
    assert_eq!(root_hash(&left, ALICE), root_hash(&right, BOB));
    assert_eq!(readable(&spans_of(&left)), readable(&spans_of(&right)));
}

/// A peer deletes the very text a mark names. The mark row survives and the
/// document simply has no formatted characters left.
#[test]
fn merge__formatting_text_a_peer_concurrently_deletes() {
    let base = seeded(SENTENCE);
    let (left, right) = (fork(&base), fork(&base));

    let formatted = edit::<Doc>(&left, device(ALICE), |doc| {
        let _id = doc.mark(4, 7, "bold", Some("true")).unwrap();
    });
    let deleted = edit::<Doc>(&right, device(BOB), |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(4), DeltaOp::Delete { delete: 3 }])
            .unwrap();
    });
    land(&left, device(ALICE), &deleted);
    land(&right, device(BOB), &formatted);

    assert_eq!(text_of(&left), "The  jumped.");
    expect(&spans_of(&left), &[("The  jumped.", NONE)]);
    assert_eq!(readable(&spans_of(&left)), readable(&spans_of(&right)));
    assert_eq!(marks_of(&left).len(), 1, "the row must be retained");
    assert_eq!(root_hash(&left, ALICE), root_hash(&right, BOB));
}

/// A mark that arrives before the text it names is inert, then correct, and the
/// outcome does not depend on which order they arrived in.
#[test]
fn merge__a_mark_arriving_before_its_text_is_inert_then_correct() {
    let base = seeded("The ");
    let (author, peer) = (fork(&base), fork(&base));

    let typed = edit::<Doc>(&author, device(ALICE), |doc| {
        let _undo = doc
            .apply_delta(&[DeltaOp::retain(4), DeltaOp::insert("fox")])
            .unwrap();
    });
    let formatted = edit::<Doc>(&author, device(ALICE), |doc| {
        let _id = doc.mark(4, 7, "bold", Some("true")).unwrap();
    });

    land(&peer, device(BOB), &formatted);
    expect(&spans_of(&peer), &[("The ", NONE)]);
    assert_eq!(
        marks_of(&peer).len(),
        1,
        "the mark must be stored even though it resolves nowhere"
    );

    land(&peer, device(BOB), &typed);
    assert_eq!(readable(&spans_of(&peer)), readable(&spans_of(&author)));
    assert_eq!(root_hash(&peer, BOB), root_hash(&author, ALICE));
}

#[test]
fn merge__is_commutative_associative_and_idempotent() {
    let base = seeded(SENTENCE);
    let writes: [(u8, usize, usize, &str, Option<&str>); 3] = [
        (1, 0, 7, "bold", Some("true")),
        (2, 4, 15, "italic", Some("true")),
        (3, 2, 9, "bold", None),
    ];
    let deltas: Vec<Vec<u8>> = writes
        .iter()
        .map(|(writer, start, end, key, value)| {
            let store = fork(&base);
            edit::<Doc>(&store, device(*writer), |doc| {
                let _id = doc.mark(*start, *end, key, *value).unwrap();
            })
        })
        .collect();

    let landed = |order: &[usize], repeat: usize| -> Vec<(String, Vec<(String, String)>)> {
        let store = fork(&base);
        for _ in 0..repeat {
            for index in order {
                land(&store, device(ALICE), &deltas[*index]);
            }
        }
        readable(&spans_of(&store))
    };
    let reference = landed(&[0, 1, 2], 1);
    assert_eq!(landed(&[2, 1, 0], 1), reference, "not commutative");
    assert_eq!(landed(&[1, 0, 2], 1), reference, "not associative");
    assert_eq!(landed(&[0, 1, 2], 3), reference, "not idempotent");
    assert!(reference.len() > 1, "the fixture produced one flat span");
}

/// Two marks minted with the same `(lamport, replica)` are one key, so one row
/// wins and neither replica is left holding a row the other does not.
#[test]
fn merge__two_marks_sharing_an_id_converge_on_one_row() {
    let base = seeded(SENTENCE);
    let (left, right) = (fork(&base), fork(&base));

    let from_left = edit::<Doc>(&left, device(ALICE), |doc| {
        let _id = doc
            .mark_with_replica(0, 7, "bold", Some("true"), 9)
            .unwrap();
    });
    let from_right = edit::<Doc>(&right, device(BOB), |doc| {
        let _id = doc
            .mark_with_replica(4, 15, "italic", Some("true"), 9)
            .unwrap();
    });
    assert_eq!(marks_of(&left)[0].id, marks_of(&right)[0].id);

    land(&left, device(ALICE), &from_right);
    land(&right, device(BOB), &from_left);
    assert_eq!(marks_of(&left).len(), 1);
    assert_eq!(marks_of(&left), marks_of(&right), "the replicas diverged");
    assert_eq!(root_hash(&left, ALICE), root_hash(&right, BOB));
}

// ── growth is visible, not acceptable ───────────────────────────────────

#[test]
fn toggling_one_range_writes_one_row_per_toggle_and_still_reads() {
    let store = seeded(SENTENCE);
    let _delta = edit::<Doc>(&store, device(ALICE), |doc| {
        for round in 0..TOGGLES {
            let value = (round % 2 == 0).then_some("true");
            let id = doc.mark(0, 7, "bold", value).unwrap();
            assert!(id.is_some(), "toggle {round} was suppressed");
        }
    });
    assert_eq!(marks_of(&store).len(), TOGGLES);
    // The final toggle turns bold off, so every character loses it and the two
    // sides of the old boundary merge back into one span.
    expect(&spans_of(&store), &[(SENTENCE, NONE)]);
}

// ── a document nested in a map ──────────────────────────────────────────

#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Notes {
    docs: UnorderedMap<String, Doc>,
}

#[test]
fn nested__two_replicas_creating_one_key_converge_on_text_and_marks() {
    let base = genesis(
        || {
            let mut state = Notes::default();
            state.docs.reassign_deterministic_id("docs");
            state
        },
        |_| (),
    );
    let (left, right) = (fork(&base), fork(&base));

    let write = |store: &Store, writer: u8, text: &str| -> Vec<u8> {
        edit::<Notes>(store, device(writer), |state| {
            let mut doc = state
                .docs
                .entry("d1".to_owned())
                .unwrap()
                .or_default()
                .unwrap();
            let _undo = doc.apply_delta(&[DeltaOp::insert(text)]).unwrap();
            let _id = doc
                .mark(0, text.chars().count(), "bold", Some("true"))
                .unwrap();
        })
    };
    let from_left = write(&left, ALICE, "alpha");
    let from_right = write(&right, BOB, "beta");

    land(&left, device(ALICE), &from_right);
    land(&right, device(BOB), &from_left);

    let spans = |store: &Store, writer: u8| -> Vec<(String, Vec<(String, String)>)> {
        read_with::<Notes, _>(store, device(writer), |state| {
            readable(&state.docs.get("d1").unwrap().unwrap().to_delta().unwrap())
        })
    };
    let rendered = spans(&left, ALICE);
    assert_eq!(
        rendered,
        spans(&right, BOB),
        "the nested documents diverged"
    );
    let text: String = rendered.iter().map(|(text, _)| text.as_str()).collect();
    assert!(
        text.contains("alpha") && text.contains("beta"),
        "a typed passage was shredded: {text}"
    );
    assert_eq!(
        rendered.len(),
        1,
        "both halves are bold, so they render as one span: {rendered:?}"
    );
    assert_eq!(root_hash(&left, ALICE), root_hash(&right, BOB));
}

// ── JSON shapes, pinned as exact strings ────────────────────────────────

#[test]
fn json__a_delta_is_quills_shape() {
    let ops = vec![
        DeltaOp::Retain {
            retain: 3,
            attributes: Some(attrs(&[("bold", Some("true"))])),
        },
        DeltaOp::Delete { delete: 2 },
        DeltaOp::Insert {
            insert: "hi".to_owned(),
            attributes: Some(attrs(&[("bold", None), ("link", Some("https://x"))])),
        },
        DeltaOp::retain(5),
    ];
    let json = serde_json::to_string(&ops).unwrap();
    assert_eq!(
        json,
        r#"[{"retain":3,"attributes":{"bold":true}},{"delete":2},{"insert":"hi","attributes":{"bold":null,"link":"https://x"}},{"retain":5}]"#
    );
    assert_eq!(serde_json::from_str::<Vec<DeltaOp>>(&json).unwrap(), ops);
}

#[test]
fn json__an_attribute_value_canonicalises_to_a_string_on_the_way_in() {
    let ops: Vec<DeltaOp> = serde_json::from_str(
        r#"[{"retain":1,"attributes":{"a":true,"b":false,"c":7,"d":-2,"e":1.5,"f":"x","g":null}}]"#,
    )
    .unwrap();
    let DeltaOp::Retain {
        attributes: Some(attributes),
        ..
    } = &ops[0]
    else {
        panic!("expected a retain with attributes")
    };
    let flat: Vec<(&str, Option<&str>)> = attributes
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_deref()))
        .collect();
    assert_eq!(
        flat,
        [
            ("a", Some("true")),
            ("b", Some("false")),
            ("c", Some("7")),
            ("d", Some("-2")),
            ("e", Some("1.5")),
            ("f", Some("x")),
            ("g", None),
        ]
    );
}

#[test]
fn json__a_span_emits_booleans_and_omits_an_empty_attribute_set() {
    let spans = vec![
        Span {
            text: "The ".to_owned(),
            attributes: BTreeMap::new(),
        },
        Span {
            text: "quick".to_owned(),
            attributes: [("bold".to_owned(), "true".to_owned())]
                .into_iter()
                .collect(),
        },
        Span {
            text: " fox".to_owned(),
            attributes: [
                ("bold".to_owned(), "true".to_owned()),
                ("comment:a7".to_owned(), "check this".to_owned()),
            ]
            .into_iter()
            .collect(),
        },
    ];
    let json = serde_json::to_string(&spans).unwrap();
    assert_eq!(
        json,
        r#"[{"text":"The "},{"text":"quick","attributes":{"bold":true}},{"text":" fox","attributes":{"bold":true,"comment:a7":"check this"}}]"#
    );
    assert_eq!(serde_json::from_str::<Vec<Span>>(&json).unwrap(), spans);
}

#[test]
fn json__a_mark_row_and_its_id_are_stable_shapes() {
    let mark = Mark {
        id: MarkId {
            lamport: 4,
            replica: 9,
        },
        start: Anchor::Start,
        end: Anchor::Char {
            id: (7, 2),
            bias: Bias::After,
        },
        key: "bold".to_owned(),
        value: Some("true".to_owned()),
    };
    assert_eq!(
        serde_json::to_string(&mark.id).unwrap(),
        r#"{"lamport":4,"replica":9}"#
    );
    assert_eq!(
        serde_json::to_string(&mark).unwrap(),
        r#"{"id":{"lamport":4,"replica":9},"start":"Start","end":{"Char":{"id":[7,2],"bias":"After"}},"key":"bold","value":"true"}"#
    );
    assert_eq!(
        serde_json::from_str::<Mark>(&serde_json::to_string(&mark).unwrap()).unwrap(),
        mark
    );
}

#[test]
fn json__an_undo_token_round_trips() {
    let token = DeltaUndo(vec![UndoStep::Mark {
        start: Anchor::Start,
        end: Anchor::End,
        key: "bold".to_owned(),
        value: None,
    }]);
    let json = serde_json::to_string(&token).unwrap();
    assert_eq!(
        json,
        r#"[{"mark":{"start":"Start","end":"End","key":"bold","value":null}}]"#
    );
    assert_eq!(serde_json::from_str::<DeltaUndo>(&json).unwrap(), token);
}

/// `MarkId` orders on the counter first and breaks ties on the replica, which is
/// the whole of the conflict-resolution rule.
#[test]
fn mark_id__orders_by_lamport_then_replica() {
    let id = |lamport, replica| MarkId { lamport, replica };
    assert!(id(2, 0) > id(1, 9_999));
    assert!(id(1, 2) > id(1, 1));
    let mut ids = vec![id(2, 1), id(1, 5), id(2, 0)];
    ids.sort_unstable();
    assert_eq!(ids, [id(1, 5), id(2, 0), id(2, 1)]);
}
