//! Peritext's nine worked examples, plus the one case the essay leaves open.
//!
//! Each quotes the essay (<https://www.inkandswitch.com/peritext/>) so the
//! expected outcome is traceable to the source rather than to a design note.
//! Every example is driven through the REAL apply path on two replicas and in
//! both delivery orders, and the spans and the Merkle roots must match: that is
//! what makes these conformance tests and not convergence tests.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use calimero_storage::address::Id;
use calimero_storage::collections::{
    DefaultMarks, DeltaOp, Expand, MarkSchema, RichText, Root, Span,
};
use calimero_storage::store::MainStorage;

mod fugue_harness;
mod rich_text_model;

use fugue_harness::{
    device, edit, entry_bytes, fork, genesis, land, read_with, root_hash, written_ids, Store,
};
use rich_text_model::{expect, readable, BOLD, NONE};

const ALICE: u8 = 1;
const BOB: u8 = 2;
const SENTENCE: &str = "The fox jumped."; // the essay's document throughout

type Doc = RichText<Essay, MainStorage>;

/// The essay's keys. `color` follows character formatting, which the essay
/// groups with "italic, underline, font, font size, and text color".
struct Essay;

impl MarkSchema for Essay {
    fn expand(prefix: &str) -> Option<Expand> {
        match prefix {
            "color" => Some(Expand::After),
            other => DefaultMarks::expand(other),
        }
    }
}

fn seeded(text: &str) -> Store {
    genesis(
        || Doc::new_with_field_name("essay"),
        |doc| {
            let _undo = doc.apply_delta(&[DeltaOp::insert(text)]).unwrap();
        },
    )
}

fn spans_of(store: &Store, writer: u8) -> Vec<Span> {
    read_with::<Doc, _>(store, device(writer), |doc| doc.to_delta().unwrap())
}

/// Alice and Bob edit a shared document without seeing each other, then each
/// receives the other's delta. Returns the spans both must agree on.
fn merged(
    text: &str,
    alice: impl FnOnce(&mut Root<Doc>),
    bob: impl FnOnce(&mut Root<Doc>),
) -> Vec<Span> {
    let base = seeded(text);
    let (left, right) = (fork(&base), fork(&base));

    let from_alice = edit::<Doc>(&left, device(ALICE), alice);
    let from_bob = edit::<Doc>(&right, device(BOB), bob);

    land(&left, device(ALICE), &from_bob);
    land(&right, device(BOB), &from_alice);

    let spans = spans_of(&left, ALICE);
    assert_eq!(
        readable(&spans),
        readable(&spans_of(&right, BOB)),
        "the two delivery orders rendered different documents"
    );

    let ids: BTreeSet<Id> = written_ids(&from_alice)
        .into_iter()
        .chain(written_ids(&from_bob))
        .collect();
    assert_eq!(
        entry_bytes(&left, &ids),
        entry_bytes(&right, &ids),
        "the replicas hold different row bytes, so their Merkle hashes differ"
    );
    assert_eq!(
        root_hash(&left, ALICE),
        root_hash(&right, BOB),
        "the replicas disagree on the Merkle root"
    );
    spans
}

const ITALIC: &[(&str, &str)] = &[("italic", "true")];
const BOLD_ITALIC: &[(&str, &str)] = &[("bold", "true"), ("italic", "true")];
const LINK: &[(&str, &str)] = &[("link", "https://x")];

fn insert(pos: usize, text: &str) -> Vec<DeltaOp> {
    vec![DeltaOp::retain(pos), DeltaOp::insert(text)]
}

/// "Inserted characters should stay in the same place relative to the context
/// in which they were inserted."
#[test]
fn peritext_1__concurrent_plain_inserts_do_not_interleave() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _undo = doc.apply_delta(&insert(4, "quick ")).unwrap();
        },
        |doc| {
            let _undo = doc.apply_delta(&insert(14, " over the dog")).unwrap();
        },
    );
    expect(&spans, &[("The quick fox jumped over the dog.", NONE)]);
}

/// "when formatting is added to the document, it applies to any text inside a
/// range between two characters, even if that text wasn't present when the
/// formatting was applied."
#[test]
fn peritext_2__insert_inside_a_bold_span_is_bold() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(0, 15, "bold", Some("true")).unwrap();
        },
        |doc| {
            let _undo = doc.apply_delta(&insert(4, "brown ")).unwrap();
        },
    );
    expect(&spans, &[("The brown fox jumped.", BOLD)]);
}

/// "A given character is either bold or non-bold, so in this case there is only
/// one reasonable outcome - the whole text should be bold". One span, not two
/// with equal attributes: the run merge is what makes the bytes agree too.
#[test]
fn peritext_3__overlapping_bold_merges_into_one_run() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(0, 7, "bold", Some("true")).unwrap();
        },
        |doc| {
            let _id = doc.mark(4, 15, "bold", Some("true")).unwrap();
        },
    );
    expect(&spans, &[("The fox jumped.", BOLD)]);
}

/// "because bold and italic can coexist on the same word, we think it is most
/// logical to apply both stylings to the word, making 'fox' both bold and
/// italic".
#[test]
fn peritext_4__bold_and_italic_overlap_gives_both() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(0, 7, "bold", Some("true")).unwrap();
        },
        |doc| {
            let _id = doc.mark(4, 15, "italic", Some("true")).unwrap();
        },
    );
    expect(
        &spans,
        &[("The ", BOLD), ("fox", BOLD_ITALIC), (" jumped.", ITALIC)],
    );
}

/// "in the region where the two formatting ranges overlap, we arbitrarily choose
/// either Alice's color or Bob's color", and "the same color is chosen for
/// everyone who views the document, so the choice needs to be deterministic."
#[test]
fn peritext_5__two_colours_on_one_overlap_pick_one_by_markid() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(0, 7, "color", Some("red")).unwrap();
        },
        |doc| {
            let _id = doc.mark(4, 15, "color", Some("blue")).unwrap();
        },
    );
    let overlap: Vec<&Span> = spans
        .iter()
        .filter(|span| span.text.contains("fox"))
        .collect();
    assert_eq!(
        overlap.len(),
        1,
        "'fox' must render as one colour: {spans:?}"
    );
    let colour = overlap[0].attributes.get("color").unwrap();
    assert!(
        colour == "red" || colour == "blue",
        "the overlap invented a colour neither user picked: {colour}"
    );
    assert_eq!(
        spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>(),
        SENTENCE
    );
    // Every character carries exactly one colour, and no character lost it.
    for span in &spans {
        assert!(span.attributes.contains_key("color"), "{span:?}");
    }
}

/// "The word 'The' was set to bold by Alice and not changed by Bob, so it should
/// be bold. The word 'fox' was set to non-bold by Alice and not changed by Bob,
/// so it should be non-bold." The contested word takes the greater `MarkId`,
/// which here is Alice's later un-bold.
#[test]
fn peritext_6__bold_then_unbold_versus_concurrent_bold() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(0, 15, "bold", Some("true")).unwrap();
            let _id = doc.unmark(4, 15, "bold").unwrap();
        },
        |doc| {
            let _id = doc.mark(8, 15, "bold", Some("true")).unwrap();
        },
    );
    expect(&spans, &[("The ", BOLD), ("fox jumped.", NONE)]);
}

/// "multiple comments can be associated with a single character in the text. We
/// can render this in the editor by showing the two highlight regions
/// overlapping." Two keys, so no flag and no branch decides it.
#[test]
fn peritext_7__overlapping_comments_both_survive() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(0, 7, "comment:alice", Some("hers")).unwrap();
        },
        |doc| {
            let _id = doc.mark(4, 15, "comment:bob", Some("his")).unwrap();
        },
    );
    expect(
        &spans,
        &[
            ("The ", &[("comment:alice", "hers")]),
            ("fox", &[("comment:alice", "hers"), ("comment:bob", "his")]),
            (" jumped.", &[("comment:bob", "his")]),
        ],
    );
}

/// "the text inserted before the bold span becomes non-bold, and the text
/// inserted after the bold span becomes bold."
#[test]
fn peritext_8__typing_at_a_bold_boundary() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(4, 14, "bold", Some("true")).unwrap();
        },
        |doc| {
            let _undo = doc.apply_delta(&insert(14, " over the dog")).unwrap();
            let _undo = doc.apply_delta(&insert(4, "quick ")).unwrap();
        },
    );
    expect(
        &spans,
        &[
            ("The quick ", NONE),
            ("fox jumped over the dog", BOLD),
            (".", NONE),
        ],
    );
}

/// "while a bold or italic span grows to include text inserted at the end of the
/// span, a link or comment span does not grow in the same way."
#[test]
fn peritext_9__typing_at_a_link_boundary() {
    let spans = merged(
        SENTENCE,
        |doc| {
            let _id = doc.mark(4, 14, "link", Some("https://x")).unwrap();
        },
        |doc| {
            let _undo = doc.apply_delta(&insert(14, " over the dog")).unwrap();
            let _undo = doc.apply_delta(&insert(4, "quick ")).unwrap();
        },
    );
    expect(
        &spans,
        &[
            ("The quick ", NONE),
            ("fox jumped", LINK),
            (" over the dog.", NONE),
        ],
    );
}

/// "If the end of a link and the end of a bold span fall on the same character,
/// and text is inserted after that character, what should happen? The most
/// consistent behavior would be for that text to be bold but not linked.
/// Microsoft Word behaves this way". The essay leaves it open; we pick Word's,
/// and it falls out of the two expands rather than out of a special case.
#[test]
fn loro_5__insert_after_bold_link_is_bold_not_linked() {
    let spans = merged(
        "Hello World",
        |doc| {
            let _id = doc.mark(0, 5, "bold", Some("true")).unwrap();
            let _id = doc.mark(0, 5, "link", Some("https://x")).unwrap();
        },
        |doc| {
            let _undo = doc.apply_delta(&insert(5, "X")).unwrap();
        },
    );
    expect(
        &spans,
        &[
            ("Hello", &[("bold", "true"), ("link", "https://x")]),
            ("X", BOLD),
            (" World", NONE),
        ],
    );
}
