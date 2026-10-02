#![no_main]
//! A peer's delta applied over a `RichText` through `Root::sync`, wrapped as the node wraps it.
//! The input is the peer-controlled part: the borsh `(delta_hlc, actions)` of a state delta.

use calimero_storage::collections::{DefaultMarks, DeltaOp, RichText, Root};
use calimero_storage::delta::StorageDelta;
use calimero_storage::env;
use calimero_storage::interface::{Action, ApplyContext};
use calimero_storage::logical_clock::HybridTimestamp;
use calimero_storage::register_crdt_merge;
use libfuzzer_sys::fuzz_target;

type Doc = RichText<DefaultMarks>;

fuzz_target!(
    init: register_crdt_merge::<Doc>(),
    |data: &[u8]| {
        let Ok((delta_hlc, actions)) = borsh::from_slice::<(HybridTimestamp, Vec<Action>)>(data)
        else {
            return;
        };
        env::reset_environment();
        let mut doc = Root::new(|| Doc::new_with_field_name("doc"));
        let _undo = doc
            .apply_delta(&[DeltaOp::insert("hello")])
            .expect("local insert");
        let _mark = doc.mark(0, 5, "bold", Some("true")).expect("local mark");
        doc.commit();

        // Writers and signer are resolved by the receiving node, never sent; an
        // unresolved author is `None`. A refused sync is skipped: its writes are discarded.
        let delta = StorageDelta::CausalActions {
            actions,
            delta_id: [0; 32],
            delta_hlc,
            effective_writers: Default::default(),
            signer_account: None,
            on_behalf_accounts: Default::default(),
        };
        let artifact = borsh::to_vec(&delta).expect("delta encodes");
        if Root::<Doc>::sync(&artifact, &ApplyContext::empty()).is_err() {
            return;
        }
        let mut doc = Root::<Doc>::fetch().expect("a synced document keeps its root");

        let text = doc.get_text().expect("text reads back");
        let _spans = doc.to_delta().expect("formatting reads back");
        let len = text.chars().count();
        // A peer may legitimately exhaust this replica's counters, so a refused
        // insert is allowed, but it must leave the text as it was.
        let typed = doc.apply_delta(&[DeltaOp::retain(len), DeltaOp::insert("!")]);
        let expected = if typed.is_ok() { format!("{text}!") } else { text };
        assert_eq!(doc.get_text().expect("text reads back"), expected);

        let len = expected.chars().count();
        let _mark = doc
            .mark(0, len, "italic", Some("true"))
            .expect("the local replica can still format");
        let spans = doc.to_delta().expect("formatting reads back");
        assert!(
            spans
                .iter()
                .all(|span| span.attributes.get("italic").map(String::as_str) == Some("true")),
            "a mark over the whole text is not shown: {spans:?}"
        );
    }
);
