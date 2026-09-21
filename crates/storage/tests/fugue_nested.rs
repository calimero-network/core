//! A `FugueText` stored as a collection VALUE, one and two levels deep.
//!
//! The document layer keeps text as `UnorderedMap<DocId, Doc>`, so the block map
//! of a nested `FugueText` has to derive the same id on every replica that
//! creates the same key. These drive that through `Interface::apply_action`.

#![allow(non_snake_case)]
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeMap, BTreeSet};

use calimero_sdk::app;
use calimero_sdk::app::Mergeable;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::{CrdtType, FugueText, LwwRegister, UnorderedMap};
use calimero_storage::delta::StorageDelta;
use calimero_storage::env;
use calimero_storage::index::Index;
use calimero_storage::register_rekey_if_supported;
use calimero_storage::store::{Key, MainStorage};
use calimero_wasm_abi::abi_type::{AbiType, TypeRegistry};
use calimero_wasm_abi::schema::{CollectionType, CrdtCollectionType, TypeDef, TypeRef};

mod fugue_harness;

use fugue_harness::{device, edit, env_for, fork, genesis, land, read_with, Store};

const ALICE: u8 = 1;
const BOB: u8 = 2;

/// One document: a title that is last-writer-wins and a collaboratively edited body.
#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable, calimero_sdk::abi::AbiType)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Doc {
    title: LwwRegister<String>,
    body: FugueText,
}

/// Shape (a), through the `#[app::state]` macro so the macro's own cascade is what registers `Doc`.
#[app::state]
#[derive(Default)]
pub struct DocsApp {
    docs: UnorderedMap<String, Doc>,
}

impl calimero_sdk::state::AppStateInit for DocsApp {
    type Return = DocsApp;
}

/// Shape (b): `map -> struct -> map -> struct -> FugueText`.
#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable, calimero_sdk::abi::AbiType)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Folder {
    docs: UnorderedMap<String, Doc>,
}

#[derive(BorshSerialize, BorshDeserialize, Default, Mergeable)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct FoldersApp {
    folders: UnorderedMap<String, Folder>,
}

fn docs_genesis() -> Store {
    DocsApp::__calimero_register_rekey();
    genesis(DocsApp::default, |_| ())
}

fn folders_genesis() -> Store {
    register_rekey_if_supported!(Folder);
    register_rekey_if_supported!(String);
    genesis(
        || {
            let mut state = FoldersApp::default();
            state.folders.reassign_deterministic_id("folders");
            state
        },
        |_| (),
    )
}

/// Type into `docs[key]`, creating the entry if this replica has not seen it.
fn type_into(store: &Store, writer: u8, key: &str, pos: usize, text: &str) -> Vec<u8> {
    edit::<DocsApp>(store, device(writer), |app| {
        let mut doc = app
            .docs
            .entry(key.to_owned())
            .unwrap()
            .or_default()
            .unwrap();
        let _minted = doc
            .body
            .insert_str_with_replica(pos, u64::from(writer), text)
            .unwrap();
    })
}

fn text_of(store: &Store, writer: u8, key: &str) -> Option<String> {
    read_with::<DocsApp, _>(store, device(writer), |app| {
        app.docs
            .get(key)
            .unwrap()
            .map(|doc| doc.body.get_text().unwrap())
    })
}

fn root_hash(store: &Store, writer: u8) -> Option<[u8; 32]> {
    env::with_runtime_env(env_for(store, device(writer)), env::root_hash)
}

/// The non-root entity ids a delta writes.
fn written_ids(delta: &[u8]) -> Vec<Id> {
    let actions = match borsh::from_slice::<StorageDelta>(delta).unwrap() {
        StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
    };
    actions
        .iter()
        .map(|action| action.id())
        .filter(|id| !id.is_root())
        .collect()
}

/// Of `ids`, the ones tagged `FugueTextBlock`, mapped to the collection they hang off.
fn block_rows(store: &Store, writer: u8, ids: &[Id]) -> BTreeMap<Id, Id> {
    env::with_runtime_env(env_for(store, device(writer)), || {
        ids.iter()
            .filter_map(|id| {
                let index = Index::<MainStorage>::get_index(*id).unwrap()?;
                (index.metadata.crdt_type == Some(CrdtType::FugueTextBlock))
                    .then(|| (*id, index.parent_id().unwrap()))
            })
            .collect()
    })
}

/// The stored bytes of every entity `ids` names, so two replicas can be compared row by row.
fn stored_bytes(store: &Store, ids: &BTreeSet<Id>) -> BTreeMap<Id, Option<Vec<u8>>> {
    ids.iter()
        .map(|id| {
            (
                *id,
                store.borrow().get(&Key::Entry(*id).to_bytes()).cloned(),
            )
        })
        .collect()
}

/// Everything two replicas must agree on after reconciling.
fn assert_converged(what: &str, a: &Store, b: &Store, ids: &BTreeSet<Id>) {
    assert_eq!(
        stored_bytes(a, ids),
        stored_bytes(b, ids),
        "{what}: the replicas hold different bytes, so their Merkle hashes differ"
    );
    assert_eq!(
        root_hash(a, ALICE),
        root_hash(b, BOB),
        "{what}: the replicas disagree on the Merkle root"
    );
}

/// Two replicas that never spoke both create `d1` and type into it.
#[test]
fn concurrent_creation_of_one_map_key_converges_and_shares_one_block_collection() {
    let base = docs_genesis();
    let (alice, bob) = (fork(&base), fork(&base));

    let from_alice = type_into(&alice, ALICE, "d1", 0, "alpha");
    let from_bob = type_into(&bob, BOB, "d1", 0, "beta");

    land(&alice, device(ALICE), &from_bob);
    land(&bob, device(BOB), &from_alice);

    let text = text_of(&alice, ALICE, "d1").unwrap();
    assert_eq!(Some(&text), text_of(&bob, BOB, "d1").as_ref());
    assert!(
        text.contains("alpha") && text.contains("beta"),
        "a typed passage was shredded: {text}"
    );

    let ids: BTreeSet<Id> = written_ids(&from_alice)
        .into_iter()
        .chain(written_ids(&from_bob))
        .collect();
    assert_converged("concurrent creation", &alice, &bob, &ids);

    // Both replicas derived one and the same block collection for `d1`.
    let rows = block_rows(&alice, ALICE, &ids.iter().copied().collect::<Vec<_>>());
    assert_eq!(rows.len(), 2, "one block row per writer: {rows:?}");
    let parents: BTreeSet<Id> = rows.values().copied().collect();
    assert_eq!(
        parents.len(),
        1,
        "the two replicas built disjoint documents: {parents:?}"
    );
    assert_eq!(
        rows,
        block_rows(&bob, BOB, &ids.iter().copied().collect::<Vec<_>>()),
        "the replicas tagged or parented the block rows differently"
    );
}

/// One replica creates the doc, the other receives it and types at the same position.
#[test]
fn typing_at_one_position_after_receiving_the_doc_keeps_passages_contiguous() {
    let base = docs_genesis();
    let (alice, bob) = (fork(&base), fork(&base));

    let seeded = type_into(&alice, ALICE, "d1", 0, "SE");
    land(&bob, device(BOB), &seeded);
    assert_eq!(text_of(&bob, BOB, "d1").as_deref(), Some("SE"));

    let from_alice = type_into(&alice, ALICE, "d1", 1, "aaaa");
    let from_bob = type_into(&bob, BOB, "d1", 1, "bbbb");

    land(&alice, device(ALICE), &from_bob);
    land(&bob, device(BOB), &from_alice);

    let text = text_of(&alice, ALICE, "d1").unwrap();
    assert_eq!(Some(&text), text_of(&bob, BOB, "d1").as_ref());
    assert!(
        text.contains("aaaa") && text.contains("bbbb"),
        "a typed passage interleaved: {text}"
    );
    assert_eq!(text.chars().count(), 10);

    let ids: BTreeSet<Id> = written_ids(&seeded)
        .into_iter()
        .chain(written_ids(&from_alice))
        .chain(written_ids(&from_bob))
        .collect();
    assert_converged("typing at one position", &alice, &bob, &ids);
}

/// Removing the map entry while a peer types into its text.
///
/// The entry is an ordinary map entity, so the removal is a tombstone reconciled
/// last-writer-wins against the peer's update: the edit resurrects the document
/// when it is written later, and loses when it is written earlier. Delivery order
/// does not change it, so both replicas agree either way.
#[test]
fn removing_the_entry_while_a_peer_types_resolves_by_write_order_not_delivery_order() {
    let base = docs_genesis();
    let seeded = type_into(&base, ALICE, "d1", 0, "SE");
    let _ignored = seeded;

    let outcome = |remove_written_first: bool, remove_delivered_first: bool| -> Option<String> {
        let (alice, bob) = (fork(&base), fork(&base));
        let mut removal = Vec::new();
        let mut typing = Vec::new();
        let mut remove = || {
            removal = edit::<DocsApp>(&alice, device(ALICE), |app| {
                let _removed = app.docs.remove("d1").unwrap();
            });
        };
        let mut r#type = || typing = type_into(&bob, BOB, "d1", 2, "typed");
        if remove_written_first {
            remove();
            r#type();
        } else {
            r#type();
            remove();
        }

        if remove_delivered_first {
            land(&bob, device(BOB), &removal);
            land(&alice, device(ALICE), &typing);
        } else {
            land(&alice, device(ALICE), &typing);
            land(&bob, device(BOB), &removal);
        }
        let (on_alice, on_bob) = (text_of(&alice, ALICE, "d1"), text_of(&bob, BOB, "d1"));
        assert_eq!(
            on_alice, on_bob,
            "the replicas disagree on whether the entry survives"
        );
        assert_eq!(root_hash(&alice, ALICE), root_hash(&bob, BOB));
        on_alice
    };

    for delivered_first in [true, false] {
        assert_eq!(
            outcome(true, delivered_first),
            Some("SEtyped".to_owned()),
            "an edit written after the removal must bring the document back"
        );
        assert_eq!(
            outcome(false, delivered_first),
            None,
            "a removal written after the edit must take the document away"
        );
    }
}

/// Two documents in one map must not share a block collection.
#[test]
fn two_documents_in_one_map_keep_separate_block_collections() {
    let base = docs_genesis();
    let store = fork(&base);

    let first = type_into(&store, ALICE, "d1", 0, "one");
    let second = type_into(&store, ALICE, "d2", 0, "two");

    assert_eq!(text_of(&store, ALICE, "d1").as_deref(), Some("one"));
    assert_eq!(text_of(&store, ALICE, "d2").as_deref(), Some("two"));

    let parents_of = |delta: &[u8]| -> BTreeSet<Id> {
        block_rows(&store, ALICE, &written_ids(delta))
            .values()
            .copied()
            .collect()
    };
    let (first, second) = (parents_of(&first), parents_of(&second));
    assert_eq!(first.len(), 1, "one block collection per document");
    assert_eq!(second.len(), 1);
    assert!(
        first.is_disjoint(&second),
        "the two documents share a block collection: {first:?}"
    );
}

/// Shape (b): the same key created independently two levels down.
#[test]
fn two_levels_of_nesting_converge_on_the_apply_path() {
    let base = folders_genesis();
    let (alice, bob) = (fork(&base), fork(&base));

    let type_deep = |store: &Store, writer: u8, text: &str| -> Vec<u8> {
        edit::<FoldersApp>(store, device(writer), |app| {
            let mut folder = app
                .folders
                .entry("f1".to_owned())
                .unwrap()
                .or_default()
                .unwrap();
            let mut doc = folder
                .docs
                .entry("d1".to_owned())
                .unwrap()
                .or_default()
                .unwrap();
            let _minted = doc
                .body
                .insert_str_with_replica(0, u64::from(writer), text)
                .unwrap();
        })
    };
    let from_alice = type_deep(&alice, ALICE, "alpha");
    let from_bob = type_deep(&bob, BOB, "beta");

    land(&alice, device(ALICE), &from_bob);
    land(&bob, device(BOB), &from_alice);

    let deep_text = |store: &Store, writer: u8| -> String {
        read_with::<FoldersApp, _>(store, device(writer), |app| {
            app.folders
                .get("f1")
                .unwrap()
                .unwrap()
                .docs
                .get("d1")
                .unwrap()
                .unwrap()
                .body
                .get_text()
                .unwrap()
        })
    };
    let text = deep_text(&alice, ALICE);
    assert_eq!(text, deep_text(&bob, BOB));
    assert!(
        text.contains("alpha") && text.contains("beta"),
        "a typed passage was shredded two levels down: {text}"
    );

    let ids: BTreeSet<Id> = written_ids(&from_alice)
        .into_iter()
        .chain(written_ids(&from_bob))
        .collect();
    assert_converged("two levels", &alice, &bob, &ids);
}
/// Sequential edits into one nested document must accumulate.
#[test]
fn repeated_edits_into_one_nested_document_accumulate() {
    let store = docs_genesis();
    let _a = type_into(&store, ALICE, "d1", 0, "one");
    let _b = type_into(&store, ALICE, "d2", 0, "two");
    let _c = type_into(&store, ALICE, "d1", 3, "three");
    assert_eq!(text_of(&store, ALICE, "d1").as_deref(), Some("onethree"));
    assert_eq!(text_of(&store, ALICE, "d2").as_deref(), Some("two"));
}

/// The ABI must describe the nested document, not flatten it to an opaque blob.
#[test]
fn the_abi_describes_a_document_map_down_to_its_text() {
    let mut registry = TypeRegistry::new();
    let map = <UnorderedMap<String, Doc> as AbiType>::type_ref(&mut registry);
    let TypeRef::Collection {
        collection,
        crdt_type,
        ..
    } = map
    else {
        panic!("expected a collection, got {map:?}")
    };
    assert_eq!(crdt_type, Some(CrdtCollectionType::UnorderedMap));
    let CollectionType::Map { value, .. } = collection else {
        panic!("expected a map")
    };
    assert_eq!(
        value,
        Box::new(TypeRef::Reference {
            ref_: "Doc".to_owned()
        })
    );

    let TypeDef::Record { fields } = registry
        .into_types()
        .remove("Doc")
        .expect("the document must be registered")
    else {
        panic!("expected the document to be a record")
    };
    let body = fields
        .into_iter()
        .find(|field| field.name == "body")
        .expect("the document must describe its body");
    let TypeRef::Collection { crdt_type, .. } = body.type_ else {
        panic!("expected the body to be a collection")
    };
    assert_eq!(crdt_type, Some(CrdtCollectionType::FugueText));
}
