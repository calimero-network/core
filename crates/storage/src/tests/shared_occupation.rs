//! Whoever writes a predictable id first on a node holds it there for good.
//!
//! A storage entity's type never changes once stored, a `SharedMember` never
//! changes its anchor, and a `Shared` write is checked against the writer set
//! already stored. So a member who writes an id a `SharedStorage` cell will use,
//! and delivers it to a node that does not hold the cell yet (a new joiner),
//! keeps that node from ever taking the real entry: the node splits from the
//! group, and where the forged entry is the cell's own writer set, the member
//! controls the cell there.
//!
//! Each test replays the cell's genesis on two fresh nodes: the group's, which
//! has the genesis before the forgery, and a joiner's, which gets the forgery
//! first. Both must end holding the same thing.

use std::collections::BTreeSet;

use borsh::to_vec;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::crdt_meta::CrdtType;
use crate::collections::{FrozenStorage, LwwRegister, Root, UnorderedMap, Vector, WriterSetCell};
use crate::entities::{ChildInfo, Data, Metadata, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{
    account_of_key, build_signed_member_action, build_signed_shared_action, pubkey_of, sign_action,
};
use crate::tests::owned_rules::{act_as, apply, key, text};

type Cell = WriterSetCell<LwwRegister<String>>;
type MapCell = WriterSetCell<UnorderedMap<String, LwwRegister<String>>>;
type ListsCell = WriterSetCell<UnorderedMap<String, Vector<LwwRegister<String>>>>;

/// Forgeries carry fixed timestamps, earlier than any genesis these tests build.
const FORGED_AT: u64 = 1_700_000_000_000_000_000;

const ALICE: u8 = 0xA1;
const MALLORY: u8 = 0xEE;

/// An action and the account its signer speaks for, as a node receives it.
type Delivery = (Action, calimero_account::AccountId);

/// What a node holds: its root hash, and at each id the stamp without its
/// signature, and the bytes.
type Held = (Option<[u8; 32]>, Vec<(Id, Option<(StorageType, Vec<u8>)>)>);

fn without_signature(mut stamp: StorageType) -> StorageType {
    match &mut stamp {
        StorageType::User { signature_data, .. }
        | StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => *signature_data = None,
        StorageType::Public | StorageType::Frozen => {}
    }
    stamp
}

/// An `Add` of `id` stamped `metadata`, signed by `signer` when the stamp is one
/// that carries a signature.
fn add(
    id: Id,
    data: Vec<u8>,
    ancestors: Vec<ChildInfo>,
    mut metadata: Metadata,
    signer: &SigningKey,
) -> Action {
    let signature = |nonce| {
        Some(SignatureData {
            signature: [0; 64],
            nonce,
            signer: Some(pubkey_of(signer)),
        })
    };
    let nonce = *metadata.updated_at;
    match &mut metadata.storage_type {
        StorageType::User { signature_data, .. }
        | StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => *signature_data = signature(nonce),
        StorageType::Public | StorageType::Frozen => {}
    }
    let mut action = Action::Add {
        id,
        data,
        ancestors,
        metadata,
    };
    let signed = sign_action(&action, signer);
    if let Action::Add { metadata, .. } = &mut action {
        match &mut metadata.storage_type {
            StorageType::User {
                signature_data: Some(sig),
                ..
            }
            | StorageType::Shared {
                signature_data: Some(sig),
                ..
            }
            | StorageType::SharedMember {
                signature_data: Some(sig),
                ..
            } => sig.signature = signed,
            _ => {}
        }
    }
    action
}

/// The actions a creator's node ships for `top` and everything beneath it,
/// parents first, read back from this node's storage.
fn shipped(top: Id, signer: &SigningKey) -> Vec<Delivery> {
    let mut out = Vec::new();
    let mut pending = vec![top];
    while let Some(id) = pending.pop() {
        let metadata = <Index<MainStorage>>::get_metadata(id)
            .expect("metadata")
            .expect("entity");
        if metadata.crdt_type == Some(CrdtType::RotationLog) {
            continue;
        }
        let data = MainStorage::storage_read(Key::Entry(id)).unwrap_or_default();
        let ancestors = <Index<MainStorage>>::get_ancestors_of(id).expect("ancestors");
        out.push((
            add(id, data, ancestors, metadata, signer),
            account_of_key(signer),
        ));
        for child in <Index<MainStorage>>::get_children_of(id).expect("children") {
            pending.push(child.id());
        }
    }
    out
}

fn ids_of(deliveries: &[Delivery]) -> Vec<Id> {
    deliveries.iter().map(|(action, _)| action.id()).collect()
}

fn ancestors_of(deliveries: &[Delivery], id: Id) -> Vec<ChildInfo> {
    deliveries
        .iter()
        .find_map(|(action, _)| match action {
            Action::Add {
                id: at, ancestors, ..
            } if *at == id => Some(ancestors.clone()),
            _ => None,
        })
        .expect("shipped")
}

/// Apply `deliveries` in order on a fresh node and return what it holds at
/// `ids`. A refusal is the behaviour under test, so each result is ignored.
fn node<'a>(deliveries: impl IntoIterator<Item = &'a Delivery>, ids: &[Id]) -> Held {
    env::reset_for_testing();
    for (action, account) in deliveries {
        let _ = apply(action.clone(), *account);
    }
    let root_hash = <Index<MainStorage>>::get_hashes_for(Id::root())
        .expect("hashes")
        .map(|(full, _)| full);
    let entities = ids
        .iter()
        .map(|id| {
            let held = <Index<MainStorage>>::get_metadata(*id)
                .expect("metadata")
                .map(|metadata| {
                    (
                        without_signature(metadata.storage_type),
                        MainStorage::storage_read(Key::Entry(*id)).unwrap_or_default(),
                    )
                });
            (*id, held)
        })
        .collect();
    (root_hash, entities)
}

/// The group has `genesis` and then receives `forged`; the joiner receives
/// `forged` first. Returns what each holds at the genesis ids.
fn group_and_joiner(genesis: &[Delivery], forged: &[Delivery]) -> (Held, Held) {
    let ids = ids_of(genesis);
    let group = node(genesis.iter().chain(forged), &ids);
    assert!(
        group.1.iter().all(|(_, held)| held.is_some()),
        "the group takes the whole genesis"
    );
    let joiner = node(forged.iter().chain(genesis), &ids);
    (group, joiner)
}

/// Alice's cell as `init` leaves it: her the one writer, a value in it.
fn alices_cell() -> (Vec<Delivery>, Id, Id) {
    env::reset_for_testing();
    let alice = key(ALICE);
    let writers: BTreeSet<_> = [act_as(&alice)].into_iter().collect();
    let mut cell = Root::new(|| Cell::new_with_field_name("doc", writers, false));
    let _ = cell.insert(text("alice's value")).expect("insert");
    let (anchor, value) = (cell.anchor(), cell.value_id());
    (shipped(anchor, &alice), anchor, value)
}

/// Mallory's own cell, at an id of her choosing under `ancestors`, with her
/// its one writer.
fn mallorys_anchor(at: Id, ancestors: Vec<ChildInfo>) -> Delivery {
    let mallory = key(MALLORY);
    let writers = [account_of_key(&mallory)].into_iter().collect();
    (
        build_signed_shared_action(true, at, vec![], writers, FORGED_AT, &mallory, ancestors),
        account_of_key(&mallory),
    )
}

fn value_bytes(value_id: Id, value: &str) -> Vec<u8> {
    to_vec(&(text(value), value_id)).expect("serialize")
}

/// The control: with no forgery, the order genesis arrives in is all there is,
/// and the group and a joiner hold the same.
#[test]
#[serial]
fn the_genesis_alone_leaves_the_group_and_a_joiner_the_same() {
    let (genesis, _, _) = alices_cell();
    let (group, joiner) = group_and_joiner(&genesis, &[]);
    assert_eq!(group, joiner);
}

/// A1: an unsigned `Public` entry at the cell's value id.
#[test]
#[serial]
fn a_public_entry_at_the_value_id_does_not_take_it() {
    let (genesis, _, value) = alices_cell();
    let mallory = key(MALLORY);
    let forged = [(
        add(
            value,
            value_bytes(value, "mallory's value"),
            ancestors_of(&genesis, value),
            Metadata::new(FORGED_AT, FORGED_AT),
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A2: a member of Mallory's own cell at Alice's value id, validly signed for
/// the anchor it names.
#[test]
#[serial]
fn a_member_of_another_anchor_at_the_value_id_does_not_take_it() {
    let (genesis, anchor, value) = alices_cell();
    let mallory = key(MALLORY);
    let her_anchor = Id::new([0x3E; 32]);
    let forged = [
        mallorys_anchor(her_anchor, ancestors_of(&genesis, anchor)),
        (
            build_signed_member_action(
                true,
                value,
                her_anchor,
                value_bytes(value, "mallory's value"),
                FORGED_AT + 1,
                &mallory,
                ancestors_of(&genesis, value),
            ),
            account_of_key(&mallory),
        ),
    ];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A3: a writer set naming only Mallory at Alice's wrapper id. Where it lands
/// first, Alice's genesis is refused and Mallory writes the value.
#[test]
#[serial]
fn a_forged_writer_set_at_the_wrapper_id_does_not_take_the_cell() {
    let (genesis, anchor, value) = alices_cell();
    let mallory = key(MALLORY);
    let writers = [account_of_key(&mallory)].into_iter().collect();
    let forged = [
        (
            build_signed_shared_action(
                true,
                anchor,
                vec![],
                writers,
                FORGED_AT,
                &mallory,
                ancestors_of(&genesis, anchor),
            ),
            account_of_key(&mallory),
        ),
        (
            build_signed_member_action(
                true,
                value,
                anchor,
                value_bytes(value, "mallory's value"),
                FORGED_AT + 1,
                &mallory,
                ancestors_of(&genesis, value),
            ),
            account_of_key(&mallory),
        ),
    ];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A3 through an ancestor: an index entry is created from the stamp an action
/// claims for a missing ancestor, which no one signs.
#[test]
#[serial]
fn a_forged_ancestor_stamp_does_not_take_the_cell() {
    let (genesis, anchor, _) = alices_cell();
    let mallory = key(MALLORY);
    let mut claimed = ancestors_of(&genesis, anchor);
    let stamp = Metadata {
        storage_type: StorageType::Shared {
            writers: crate::tests::common::writers_of([account_of_key(&mallory)]),
            signature_data: None,
        },
        ..Metadata::new(FORGED_AT, FORGED_AT)
    };
    claimed.insert(0, ChildInfo::new(anchor, [0; 32], stamp));
    let forged = [(
        add(
            Id::new([0x5A; 32]),
            vec![1],
            claimed,
            Metadata::new(FORGED_AT, FORGED_AT),
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    // The forged child is `Public`, and the group, which holds the ancestor
    // already, links it there, so the root hashes differ by it. What matters is
    // what each node holds of the cell.
    assert_eq!(group.1, joiner.1, "the joiner must end where the group is");
}

/// The same through an ancestor, at the value id.
#[test]
#[serial]
fn a_forged_ancestor_stamp_does_not_take_the_value_id() {
    let (genesis, _, value) = alices_cell();
    let mallory = key(MALLORY);
    let mut claimed = ancestors_of(&genesis, value);
    claimed.insert(
        0,
        ChildInfo::new(value, [0; 32], Metadata::new(FORGED_AT, FORGED_AT)),
    );
    let forged = [(
        add(
            Id::new([0x5B; 32]),
            vec![1],
            claimed,
            Metadata::new(FORGED_AT, FORGED_AT),
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    // The forged child is `Public`, and the group, which holds the ancestor
    // already, links it there, so the root hashes differ by it. What matters is
    // what each node holds of the cell.
    assert_eq!(group.1, joiner.1, "the joiner must end where the group is");
}

/// Alice's cell holding a map, with one entry she wrote in it. The map's own
/// entity sits beside the cell in the tree, so both subtrees ship.
fn alices_map_cell() -> (Vec<Delivery>, Id) {
    env::reset_for_testing();
    let alice = key(ALICE);
    let writers: BTreeSet<_> = [act_as(&alice)].into_iter().collect();
    let mut cell = Root::new(|| MapCell::new_with_field_name("doc", writers, false));
    let map = cell.get_mut().expect("map");
    let _ = map
        .insert("k".to_owned(), text("alice's entry"))
        .expect("insert");
    let map_id = map.element().id();
    let mut genesis = shipped(cell.anchor(), &alice);
    genesis.extend(shipped(map_id, &alice));
    (genesis, crate::collections::compute_id(map_id, b"k"))
}

/// A1 one level down: a `Public` entry at the id of an entry in the cell's map.
#[test]
#[serial]
fn a_public_entry_at_an_entry_id_under_the_value_does_not_take_it() {
    let (genesis, entry) = alices_map_cell();
    assert!(ids_of(&genesis).contains(&entry));
    let mallory = key(MALLORY);
    let forged = [(
        add(
            entry,
            to_vec(&((text("mallory's entry"), "k".to_owned()), entry)).expect("serialize"),
            ancestors_of(&genesis, entry),
            Metadata::new(FORGED_AT, FORGED_AT),
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A snapshot is checked by the same rule, except that it carries the writer
/// set a cell has now, so a wrapper is held only to being `Shared`.
#[test]
#[serial]
fn a_snapshot_leaf_at_a_cell_id_must_be_the_cells_own() {
    type MainInterface = crate::interface::Interface<MainStorage>;
    let leaf = |action: &Action| {
        let Action::Add {
            id, data, metadata, ..
        } = action
        else {
            panic!("an add");
        };
        (*id, data.clone(), metadata.clone())
    };
    let (genesis, _, value) = alices_cell();
    for (action, _) in &genesis {
        let (id, data, metadata) = leaf(action);
        MainInterface::verify_snapshot_entity_signature(id, &data, &metadata)
            .expect("the cell's own leaf");
    }

    let mallory = key(MALLORY);
    let foreign_member = build_signed_member_action(
        true,
        value,
        Id::new([0x3E; 32]),
        value_bytes(value, "mallory's value"),
        FORGED_AT,
        &mallory,
        vec![],
    );
    let (id, data, metadata) = leaf(&foreign_member);
    assert!(MainInterface::verify_snapshot_entity_signature(id, &data, &metadata).is_err());
    assert!(MainInterface::verify_snapshot_member_signature(id, &data, &metadata).is_err());
    let public = Metadata::new(FORGED_AT, FORGED_AT);
    assert!(MainInterface::verify_snapshot_entity_signature(value, &data, &public).is_err());
}

/// A nested cell holding lists. The cell's id is random, and so is every id a
/// push mints: each must still be one merge lets the cell's entities hold, or
/// no peer takes them.
#[test]
#[serial]
fn a_nested_cell_and_the_lists_in_it_reach_a_fresh_node() {
    env::reset_for_testing();
    let alice = key(ALICE);
    let writers: BTreeSet<_> = [act_as(&alice)].into_iter().collect();
    let mut cell = Root::new(|| ListsCell::new(writers, false));
    let lists = cell.get_mut().expect("lists");
    let mut list = Vector::new();
    list.push(text("before")).expect("push");
    let _ = lists.insert("k".to_owned(), list).expect("insert");
    let mut list = lists.get("k").expect("get").expect("k").into_inner();
    list.push(text("after")).expect("push");
    crate::tests::common::assert_every_shared_entity_is_bound();

    // The value's collections sit beside the cell in the tree, so ship all of it.
    let mut genesis = Vec::new();
    for top in <Index<MainStorage>>::get_children_of(Id::root()).expect("children") {
        genesis.extend(shipped(top.id(), &alice));
    }
    let (group, joiner) = group_and_joiner(&genesis, &[]);
    assert_eq!(group, joiner);
}

/// A plain `Public` map at its field id, with one entry: whatever a node stores
/// at that id first, it keeps the storage type of for good.
fn alices_public_map() -> (Vec<Delivery>, Id) {
    env::reset_for_testing();
    let alice = key(ALICE);
    let _ = act_as(&alice);
    let mut items =
        Root::new(|| UnorderedMap::<String, LwwRegister<String>>::new_with_field_name("items"));
    let _ = items
        .insert("k".to_owned(), text("an item"))
        .expect("insert");
    let field = items.element().id();
    (shipped(field, &alice), field)
}

/// A writer set at a `Public` collection's field id. No cell derived the id, and
/// every cell's wrapper, a nested one's included, lives at a tagged id, so no
/// `Shared` entity belongs there.
#[test]
#[serial]
fn a_forged_stamp_at_a_public_collection_field_id_does_not_take_it() {
    let (genesis, field) = alices_public_map();
    let forged = [mallorys_anchor(field, ancestors_of(&genesis, field))];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A member of Mallory's own cell, at a cell id of its own, at a `Public`
/// collection's field id: a member lives only at an id bound to its anchor.
#[test]
#[serial]
fn a_member_at_a_public_collection_field_id_does_not_take_it() {
    let (genesis, field) = alices_public_map();
    let mallory = key(MALLORY);
    let her_writers = crate::tests::common::writers_of([account_of_key(&mallory)]);
    let her_anchor = crate::collections::cell_id(Id::new([0x3E; 32]), &her_writers);
    let forged = [
        mallorys_anchor(her_anchor, ancestors_of(&genesis, field)),
        (
            build_signed_member_action(
                true,
                field,
                her_anchor,
                to_vec(&field).expect("serialize"),
                FORGED_AT + 1,
                &mallory,
                ancestors_of(&genesis, field),
            ),
            account_of_key(&mallory),
        ),
    ];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    // Mallory's own cell is hers to create, and the group, which holds the
    // field already, links it beside it, so the root hashes differ by it. What
    // matters is what each node holds at the field.
    assert_eq!(group.1, joiner.1, "the joiner must end where the group is");
}

/// A self-consistent content-addressed entry at a `Public` collection's field
/// id. It would have to sit at `compute_id(parent, key)`, and a field id is a
/// `compute_collection_id`: the two hash under different domain separators, so
/// no parent and key reach a field id.
#[test]
#[serial]
fn a_frozen_entry_at_a_public_collection_field_id_does_not_take_it() {
    let (genesis, field) = alices_public_map();
    let value = to_vec(&crate::collections::FrozenValue("a forgery".to_owned())).expect("value");
    let key_of_forgery: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&value).into();
    let mut data = value;
    data.extend_from_slice(&key_of_forgery);
    data.extend_from_slice(field.as_bytes());
    let mallory = key(MALLORY);
    let forged = [(
        add(
            field,
            data,
            ancestors_of(&genesis, field),
            Metadata {
                storage_type: StorageType::Frozen,
                crdt_type: Some(CrdtType::FrozenStorage),
                ..Metadata::new(FORGED_AT, FORGED_AT)
            },
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A `Public` entity at a `Public` collection's field id is the same storage
/// type, so nothing splits: the two converge by timestamp like any update.
#[test]
#[serial]
fn a_public_entry_at_a_public_collection_field_id_converges() {
    let (genesis, field) = alices_public_map();
    let mallory = key(MALLORY);
    let forged = [(
        add(
            field,
            to_vec(&field).expect("serialize"),
            ancestors_of(&genesis, field),
            Metadata::new(FORGED_AT, FORGED_AT),
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// An owned entry at a `Public` collection's field id: an owned entry lives
/// only at an owner-derived id, which a field id is not.
#[test]
#[serial]
fn an_owned_entry_at_a_public_collection_field_id_does_not_take_it() {
    let (genesis, field) = alices_public_map();
    let mallory = key(MALLORY);
    let forged = [(
        add(
            field,
            to_vec(&field).expect("serialize"),
            ancestors_of(&genesis, field),
            Metadata {
                storage_type: StorageType::User {
                    rules: crate::entities::EntryRules::OWNED,
                    owner: account_of_key(&mallory),
                    signature_data: None,
                },
                ..Metadata::new(FORGED_AT, FORGED_AT)
            },
            &mallory,
        ),
        account_of_key(&mallory),
    )];
    let (group, joiner) = group_and_joiner(&genesis, &forged);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}

/// A content-addressed entry: every node recomputes the key from the value, so
/// a forgery must at least be self-consistent. Whether its id must match its key
/// is what this asks.
fn frozen_forgery() -> (Vec<Delivery>, Delivery) {
    env::reset_for_testing();
    let alice = key(ALICE);
    let _ = act_as(&alice);
    let mut items = Root::new(FrozenStorage::<String>::new);
    let hash = items.insert("the real one".to_owned()).expect("insert");
    let entry = crate::collections::compute_id(items.inner_id(), &hash);
    let genesis = shipped(items.inner_id(), &alice);
    let crdt_type = <Index<MainStorage>>::get_metadata(entry)
        .expect("metadata")
        .expect("entry")
        .crdt_type;

    let value = to_vec(&crate::collections::FrozenValue("a forgery".to_owned())).expect("value");
    let key_of_forgery: [u8; 32] = <sha2::Sha256 as sha2::Digest>::digest(&value).into();
    let mut data = value;
    data.extend_from_slice(&key_of_forgery);
    data.extend_from_slice(entry.as_bytes());
    let mallory = key(MALLORY);
    let at = env::time_now();
    let forged = (
        add(
            entry,
            data,
            ancestors_of(&genesis, entry),
            Metadata {
                storage_type: StorageType::Frozen,
                crdt_type,
                ..Metadata::new(at, at)
            },
            &mallory,
        ),
        account_of_key(&mallory),
    );
    (genesis, forged)
}

#[test]
#[serial]
fn a_self_consistent_forgery_at_a_content_addressed_id_does_not_take_it() {
    let (genesis, forged) = frozen_forgery();
    let (group, joiner) = group_and_joiner(&genesis, &[forged]);
    assert_eq!(group, joiner, "the joiner must end where the group is");
}
