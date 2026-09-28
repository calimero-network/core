//! An owned collection nested in a writer-set cell syncs, and answers to both.
//!
//! A `SharedStorage` cell's value subtree lives at ids bound to the cell's
//! anchor, and an owned entry lives at an id bound to its owner. An owned
//! collection inside a cell's value (an `Authored` map, an `AuthoredVector`, a
//! `UserStorage`, ...) needs both at once. When the owned id kept the cell's
//! tag and dropped most of its binding, the author's node stored the entry and
//! every peer refused it, so the nodes never met again.
//!
//! The first tests write into each owned type held in a cell on an author's
//! node, replay every entity it stores on a fresh peer, and require the peer to
//! take all of it: the same entities and the same root hash. The rest play a
//! patched peer against the rule that makes it safe: an entry there is its
//! owner's, and its owner writes the cell.

use std::collections::BTreeSet;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::crdt_meta::{CrdtType, Mergeable};
use crate::collections::{
    cell_id, cell_value_id, compute_collection_id, compute_id, owned_entry_id, Authored,
    AuthoredSortedMap, AuthoredVector, IndexValue, Indexed, IndexedMap, LwwRegister, Root,
    SharedStorage, UnorderedMap, UserStorage, WriteOnce, WriterSetCell,
};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{MainInterface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{
    account_of_key, assert_every_owned_entry_is_bound, assert_every_shared_entity_is_bound,
    pubkey_of, sign_action, writers_of,
};
use crate::tests::owned_rules::{act_as, apply, entry_bytes, key, signed, text};

type Posts = Authored<UnorderedMap<String, String>>;

const ALICE: u8 = 0xA1;
const BOB: u8 = 0xB0;
const CAROL: u8 = 0xC0;
const MALLORY: u8 = 0xEE;

/// Forgeries carry fixed timestamps, earlier than any write these tests make.
const FORGED_AT: u64 = 1_700_000_000_000_000_000;

/// An action and the account its signer speaks for, as a peer receives it.
type Delivery = (Action, AccountId);

/// What a node holds: its root hash, and every entity's id, stamp without its
/// signature, and bytes.
type Held = (Option<[u8; 32]>, Vec<(Id, StorageType, Vec<u8>)>);

fn account(seed: u8) -> AccountId {
    account_of_key(&key(seed))
}

fn signature_of(stamp: &mut StorageType) -> Option<&mut Option<SignatureData>> {
    match stamp {
        StorageType::User { signature_data, .. }
        | StorageType::Shared { signature_data, .. }
        | StorageType::SharedMember { signature_data, .. } => Some(signature_data),
        StorageType::Public | StorageType::Frozen => None,
    }
}

/// `action` signed by `signer`, when its stamp is one that carries a signature.
fn signed_by(mut action: Action, signer: &SigningKey) -> Action {
    let Action::Add { metadata, .. } = &mut action else {
        return action;
    };
    let nonce = *metadata.updated_at;
    let Some(signature_data) = signature_of(&mut metadata.storage_type) else {
        return action;
    };
    *signature_data = Some(SignatureData {
        signature: [0; 64],
        nonce,
        signer: Some(pubkey_of(signer)),
    });
    let signature = sign_action(&action, signer);
    if let Action::Add { metadata, .. } = &mut action {
        if let Some(Some(sig)) = signature_of(&mut metadata.storage_type) {
            sig.signature = signature;
        }
    }
    action
}

fn is_owned(action: &Action) -> bool {
    matches!(
        action,
        Action::Add { metadata, .. } if matches!(metadata.storage_type, StorageType::User { .. })
    )
}

/// Every entity the author's node stores, each as the `Add` its writer ships:
/// an owned entry signed by its owner, anything else by `creator`. Parents come
/// first, and owned entries last, as the author wrote them: after the cell whose
/// value they answer to.
fn shipped(creator: &SigningKey, owners: &[&SigningKey]) -> Vec<Delivery> {
    let mut out = Vec::new();
    let mut pending = vec![Id::root()];
    while let Some(id) = pending.pop() {
        let metadata = <Index<MainStorage>>::get_metadata(id)
            .expect("metadata")
            .expect("entity");
        if metadata.crdt_type == Some(CrdtType::RotationLog) {
            continue;
        }
        let signer = match &metadata.storage_type {
            StorageType::User { owner, .. } => *owners
                .iter()
                .find(|sk| account_of_key(sk) == *owner)
                .expect("an owner's key"),
            _ => creator,
        };
        let action = Action::Add {
            id,
            data: MainStorage::storage_read(Key::Entry(id)).unwrap_or_default(),
            ancestors: <Index<MainStorage>>::get_ancestors_of(id).expect("ancestors"),
            metadata,
        };
        out.push((signed_by(action, signer), account_of_key(signer)));
        for child in <Index<MainStorage>>::get_children_of(id).expect("children") {
            pending.push(child.id());
        }
    }
    out.sort_by_key(|(action, _)| is_owned(action));
    out
}

fn held() -> Held {
    let root = <Index<MainStorage>>::get_hashes_for(Id::root())
        .expect("hashes")
        .map(|(full, _)| full);
    let mut entities = Vec::new();
    let mut pending = vec![Id::root()];
    while let Some(id) = pending.pop() {
        let Some(metadata) = <Index<MainStorage>>::get_metadata(id).expect("metadata") else {
            continue;
        };
        let mut stamp = metadata.storage_type;
        if let Some(signature_data) = signature_of(&mut stamp) {
            *signature_data = None;
        }
        let bytes = MainStorage::storage_read(Key::Entry(id)).unwrap_or_default();
        entities.push((id, stamp, bytes));
        for child in <Index<MainStorage>>::get_children_of(id).expect("children") {
            pending.push(child.id());
        }
    }
    entities.sort_by_key(|(id, _, _)| *id);
    (root, entities)
}

/// Apply `deliveries` in order on a fresh node, requiring each to be taken.
fn peer(deliveries: &[Delivery]) -> Result<(), (Id, StorageError)> {
    env::reset_for_testing();
    for (action, account) in deliveries {
        apply(action.clone(), *account).map_err(|error| (action.id(), error))?;
    }
    Ok(())
}

/// Apply `deliveries` in order on a fresh node, each refusal being the
/// behaviour under test, and return what the node holds.
fn node<'a>(deliveries: impl IntoIterator<Item = &'a Delivery>) -> Held {
    env::reset_for_testing();
    for (action, account) in deliveries {
        let _ = apply(action.clone(), *account);
    }
    held()
}

/// Alice and Bob, the cells' writers, acting as Alice.
fn alice_and_bob() -> BTreeSet<AccountId> {
    env::reset_for_testing();
    [act_as(&key(BOB)), act_as(&key(ALICE))]
        .into_iter()
        .collect()
}

/// A `SharedStorage` cell held by `Root`, with Alice and Bob its writers, as
/// `init` leaves it.
fn alice_and_bob_cell<T>() -> Root<SharedStorage<T>>
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + Data,
{
    let writers = alice_and_bob();
    Root::new(|| SharedStorage::new_with_field_name("doc", writers, false))
}

/// Alice writes into the owned collection held in the cell, then Bob does. A
/// peer that takes every entity the author stores reads what the author reads,
/// holds the same entities and ends on the same root hash.
fn both_writers_reach_a_peer<T, R>(write: impl Fn(&mut T, &str), read: impl Fn(&T) -> R)
where
    T: BorshSerialize + BorshDeserialize + Mergeable + Default + Data,
    R: PartialEq + core::fmt::Debug,
{
    let mut cell = alice_and_bob_cell::<T>();
    write(cell.get_mut().expect("value"), "alice's");
    let _ = act_as(&key(BOB));
    write(cell.get_mut().expect("value"), "bob's");
    assert_every_owned_entry_is_bound();
    assert_every_shared_entity_is_bound();
    let read_by_author = read(cell.get().expect("value"));
    let authored = held();
    let (alice, bob) = (key(ALICE), key(BOB));
    let deliveries = shipped(&alice, &[&alice, &bob]);
    let owners: BTreeSet<_> = deliveries
        .iter()
        .filter_map(|(action, account)| is_owned(action).then_some(*account))
        .collect();
    assert_eq!(owners.len(), 2, "each writer's entry ships");

    peer(&deliveries).expect("the peer takes every entity");
    assert_eq!(held(), authored, "the peer holds what the author holds");
    assert_every_owned_entry_is_bound();
    assert_every_shared_entity_is_bound();
    let fetched = Root::<SharedStorage<T>>::fetch().expect("the peer holds the root");
    assert_eq!(read(fetched.get().expect("value")), read_by_author);
}

/// The same through the cell every other shared type is built on.
#[test]
#[serial]
fn an_authored_map_in_a_writer_set_cell_reaches_a_peer() {
    let writers = alice_and_bob();
    let mut cell = Root::new(|| WriterSetCell::<Posts>::new_with_field_name("doc", writers, false));
    cell.get_mut()
        .expect("value")
        .insert("k".to_owned(), "alice's".to_owned())
        .expect("alice writes");
    let _ = act_as(&key(BOB));
    cell.get_mut()
        .expect("value")
        .insert("k".to_owned(), "bob's".to_owned())
        .expect("bob writes");
    let authored = held();
    let (alice, bob) = (key(ALICE), key(BOB));

    peer(&shipped(&alice, &[&alice, &bob])).expect("the peer takes every entity");
    assert_eq!(held(), authored, "the peer holds what the author holds");
    assert_every_owned_entry_is_bound();
}

#[test]
#[serial]
fn an_authored_map_in_a_cell_reaches_a_peer() {
    type Map = Authored<UnorderedMap<String, LwwRegister<String>>>;
    both_writers_reach_a_peer(
        |map: &mut Map, value| {
            // Two keys of one owner: in a cell their ids used to be one.
            for key in ["k", "k2"] {
                map.insert(key.to_owned(), text(value)).expect("insert");
            }
        },
        |map: &Map| {
            let mut entries: Vec<_> = map
                .entries_with_owners()
                .expect("entries")
                .into_iter()
                .map(|(owner, key, value)| (owner, key, value.get().clone()))
                .collect();
            entries.sort();
            assert_eq!(entries.len(), 4, "two keys per writer");
            entries
        },
    );
}

#[test]
#[serial]
fn an_authored_sorted_map_in_a_cell_reaches_a_peer() {
    type Map = AuthoredSortedMap<String, String>;
    both_writers_reach_a_peer(
        |map: &mut Map, value| {
            map.insert("k".to_owned(), value.to_owned())
                .expect("insert");
        },
        |map: &Map| {
            let entries = map.entries_with_owners().expect("entries");
            assert_eq!(entries.len(), 2);
            entries
        },
    );
}

#[test]
#[serial]
fn an_authored_vector_in_a_cell_reaches_a_peer() {
    type List = AuthoredVector<LwwRegister<String>>;
    both_writers_reach_a_peer(
        |list: &mut List, value| {
            let _ = list.push(text(value)).expect("push");
        },
        |list: &List| {
            let mut values: Vec<_> = list
                .iter()
                .expect("iter")
                .map(|value| value.get().clone())
                .collect();
            values.sort();
            assert_eq!(values, ["alice's", "bob's"]);
            values
        },
    );
}

/// A value an `IndexedMap` can index.
#[derive(
    BorshSerialize, BorshDeserialize, Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd,
)]
struct Tagged {
    tag: String,
}

impl Indexed for Tagged {
    const INDEXES: &'static [&'static str] = &["tag"];

    fn index_keys(&self, index: usize, out: &mut Vec<Vec<u8>>) {
        if index == 0 {
            self.tag.encode_index(out);
        }
    }
}

#[test]
#[serial]
fn an_authored_indexed_map_in_a_cell_reaches_a_peer() {
    type Map = Authored<IndexedMap<String, Tagged>>;
    both_writers_reach_a_peer(
        |map: &mut Map, value| {
            let tagged = Tagged {
                tag: value.to_owned(),
            };
            map.insert("k".to_owned(), tagged).expect("insert");
        },
        |map: &Map| {
            let found = map.query("tag").eq("bob's").count().expect("count");
            let mut entries = map.entries_with_owners().expect("entries");
            entries.sort();
            assert_eq!((entries.len(), found), (2, 1));
            entries
        },
    );
}

#[test]
#[serial]
fn a_write_once_map_in_a_cell_reaches_a_peer() {
    type Map = WriteOnce<UnorderedMap<String, String>>;
    both_writers_reach_a_peer(
        |map: &mut Map, value| {
            map.insert("k".to_owned(), value.to_owned())
                .expect("insert");
        },
        |map: &Map| {
            let mut entries = map.entries_with_owners().expect("entries");
            entries.sort();
            assert_eq!(entries.len(), 2);
            entries
        },
    );
}

#[test]
#[serial]
fn user_storage_in_a_cell_reaches_a_peer() {
    type Slots = UserStorage<LwwRegister<String>>;
    both_writers_reach_a_peer(
        |slots: &mut Slots, value| {
            let _ = slots.insert(text(value)).expect("insert");
        },
        |slots: &Slots| {
            let mut entries: Vec<_> = slots
                .entries()
                .expect("entries")
                .map(|(owner, value)| (owner, value.get().clone()))
                .collect();
            entries.sort();
            assert_eq!(entries.len(), 2);
            entries
        },
    );
}

// ---------------------------------------------------------------------------
// A patched peer
// ---------------------------------------------------------------------------

fn posts_id(posts: &Posts) -> Id {
    let inner: &UnorderedMap<String, String> = posts;
    inner.id()
}

/// Alice's and Bob's cell holding `Posts`, with Alice's post at `key`, shipped.
/// Returns the deliveries, the id of the map its entries are children of, and
/// the cell's anchor.
fn alices_post(key_: &str) -> (Vec<Delivery>, Id, Id) {
    let mut cell = alice_and_bob_cell::<Posts>();
    let posts = cell.get_mut().expect("value");
    posts
        .insert(key_.to_owned(), "alice's".to_owned())
        .expect("insert");
    let map = posts_id(posts);
    let anchor = MainInterface::cell_anchor_of(map)
        .expect("read")
        .expect("the map is in the cell");
    let alice = key(ALICE);
    (shipped(&alice, &[&alice]), map, anchor)
}

/// The id of a map in a cell other than Alice's and Bob's, as a writer of that
/// cell derives it.
fn another_cells_map() -> Id {
    let anchor = cell_id(Id::new([0x3E; 32]), &writers_of([account(MALLORY)]));
    compute_collection_id(Some(cell_value_id(anchor)), "posts")
}

/// A signed owned `Add` of `(key, value)` at `id` under `parent`, stamped
/// `owner` and signed by the key seeded `signer`.
fn owned_add(
    parent: Id,
    id: Id,
    key_: &str,
    value: &str,
    owner: AccountId,
    signer: u8,
) -> Delivery {
    let data = entry_bytes(id, &key_.to_owned(), &value.to_owned());
    let action = signed(
        move |metadata| Action::Add {
            id,
            data,
            ancestors: vec![
                ChildInfo::new(parent, [0; 32], Metadata::default()),
                ChildInfo::new(Id::root(), [0; 32], Metadata::default()),
            ],
            metadata,
        },
        owner,
        EntryRules::OWNED,
        &key(signer),
        FORGED_AT,
    );
    (action, account(signer))
}

/// An owned entry's id in the map at `parent`, for `key` and `owner`.
fn entry_id(parent: Id, key_: &str, owner: AccountId) -> Id {
    owned_entry_id(compute_id(parent, key_.as_bytes()), &owner)
}

/// Refused on a node holding `genesis`, leaving it exactly as it was.
fn refused_after(genesis: &[Delivery], forged: &Delivery) -> StorageError {
    let before = node(genesis);
    let refusal = apply(forged.0.clone(), forged.1).expect_err("the node refuses it");
    assert_eq!(held(), before, "the refusal leaves the node as it was");
    refusal
}

fn refused_because(refusal: &StorageError, why: &str) -> bool {
    matches!(refusal, StorageError::ActionNotAllowed(reason) if reason.contains(why))
}

#[test]
#[serial]
fn an_owner_outside_the_writer_set_cannot_add_an_entry() {
    let (genesis, map, _) = alices_post("k");

    // On Carol's own node her write is refused before it is stored.
    let mut cell = alice_and_bob_cell::<Posts>();
    let _ = act_as(&key(CAROL));
    let refused = cell
        .get_mut()
        .expect("value")
        .insert("k".to_owned(), "carol's".to_owned());
    assert!(refused.is_err(), "an honest node does not store it");
    assert_every_owned_entry_is_bound();

    // And a peer refuses the entry her patched node sends, at her own id.
    let carol = account(CAROL);
    let forged = owned_add(map, entry_id(map, "k", carol), "k", "carol's", carol, CAROL);
    let refusal = refused_after(&genesis, &forged);
    assert!(refused_because(&refusal, "writers"), "{refusal:?}");
}

#[test]
#[serial]
fn a_writer_cannot_write_another_writers_entry() {
    let (genesis, map, _) = alices_post("k");
    let alice = account(ALICE);
    // Bob signs an entry stamped Alice's, at Alice's id for a key she has not
    // written yet.
    let forged = owned_add(map, entry_id(map, "k2", alice), "k2", "bob's", alice, BOB);
    let refusal = refused_after(&genesis, &forged);
    assert!(
        matches!(refusal, StorageError::InvalidSignature),
        "{refusal:?}"
    );
}

#[test]
#[serial]
fn an_entry_bound_to_another_cell_is_refused_here() {
    let (genesis, map, _) = alices_post("k");
    let mallory = account(MALLORY);

    // Bound to Mallory's own cell, named as a child of Alice's map.
    let theirs = entry_id(another_cells_map(), "m", mallory);
    let forged = owned_add(map, theirs, "m", "mallory's", mallory, MALLORY);
    let refusal = refused_after(&genesis, &forged);
    assert!(refused_because(&refusal, "bound"), "{refusal:?}");

    // Bound to Alice's cell, which Mallory does not write.
    let here = entry_id(map, "m", mallory);
    let forged = owned_add(map, here, "m", "mallory's", mallory, MALLORY);
    let refusal = refused_after(&genesis, &forged);
    assert!(refused_because(&refusal, "writers"), "{refusal:?}");

    // Bound to a cell whose value this node does not hold, so no writer set
    // can be resolved for it: refused until the cell arrives.
    let elsewhere = another_cells_map();
    let forged = owned_add(
        elsewhere,
        entry_id(elsewhere, "m", mallory),
        "m",
        "mallory's",
        mallory,
        MALLORY,
    );
    let refusal = refused_after(&genesis, &forged);
    assert!(refused_because(&refusal, "value"), "{refusal:?}");
}

/// Alice's post at `k` is predictable. Whoever reaches a new joiner first with
/// an entity at its id must not keep Alice's own write off that node.
#[test]
#[serial]
fn a_squat_on_an_owners_id_does_not_take_it() {
    let (genesis, map, anchor) = alices_post("k");
    let target = entry_id(map, "k", account(ALICE));
    assert!(genesis.iter().any(|(action, _)| action.id() == target));

    let (mallory, bob) = (account(MALLORY), account(BOB));
    let data = entry_bytes(target, &"k".to_owned(), &"squat".to_owned());
    let under_map = vec![ChildInfo::new(map, [0; 32], Metadata::default())];
    let member = Metadata {
        storage_type: StorageType::SharedMember {
            anchor,
            signature_data: None,
        },
        ..Metadata::new(FORGED_AT, FORGED_AT)
    };
    let squats = [
        // Mallory's own entry at Alice's id, named in another cell.
        owned_add(another_cells_map(), target, "k", "squat", mallory, MALLORY),
        // Bob's own entry at Alice's id, in the cell both write.
        owned_add(map, target, "k", "squat", bob, BOB),
        // A member of the cell, signed by one of its writers.
        (
            signed_by(
                Action::Add {
                    id: target,
                    data: data.clone(),
                    ancestors: under_map.clone(),
                    metadata: member,
                },
                &key(BOB),
            ),
            bob,
        ),
        // A `Public` entry, which nobody signs.
        (
            Action::Add {
                id: target,
                data,
                ancestors: under_map,
                metadata: Metadata::new(FORGED_AT, FORGED_AT),
            },
            bob,
        ),
    ];
    let group = node(&genesis);
    for squat in &squats {
        let joiner = node([squat].into_iter().chain(&genesis));
        assert_eq!(joiner, group, "the joiner must end where the group is");
    }
}

#[test]
#[serial]
fn a_snapshot_leaf_is_held_to_both_bindings() {
    let (genesis, map, _) = alices_post("k");
    let target = entry_id(map, "k", account(ALICE));
    let leaf = |(action, _): &Delivery| match action {
        Action::Add {
            id, data, metadata, ..
        } => (*id, data.clone(), metadata.clone()),
        other => panic!("only adds are shipped, got {other:?}"),
    };
    let (id, data, metadata) = genesis
        .iter()
        .find(|(action, _)| action.id() == target)
        .map(leaf)
        .expect("alice's entry ships");
    let verify = |parent, id, metadata: &Metadata| {
        MainInterface::verify_snapshot_entity_signature(id, parent, &data, metadata)
    };
    verify(Some(map), id, &metadata).expect("alice's own leaf");
    assert!(
        verify(None, id, &metadata).is_err(),
        "a leaf naming no parent"
    );
    assert!(
        verify(Some(another_cells_map()), id, &metadata).is_err(),
        "a leaf naming a parent in another cell"
    );

    let squat = owned_add(
        another_cells_map(),
        target,
        "k",
        "squat",
        account(MALLORY),
        MALLORY,
    );
    let (id, _, metadata) = leaf(&squat);
    assert!(
        verify(Some(map), id, &metadata).is_err(),
        "Mallory at Alice's id"
    );
    assert!(verify(Some(another_cells_map()), id, &metadata).is_err());
}

/// Two owners' entries at one key of a map in a cell are two entities, one
/// owner's entries at two keys are two, one key's entries share a trie bucket,
/// and nothing derived beneath an owned entry claims the cell.
#[test]
fn an_owned_id_in_a_cell_is_bound_to_its_owner_and_its_cell() {
    let map = compute_collection_id(Some(cell_value_id(Id::new([7; 32]))), "posts");
    let slot = compute_id(map, b"k");
    let (alice, bob) = (account(ALICE), account(BOB));
    let (hers, his) = (owned_entry_id(slot, &alice), owned_entry_id(slot, &bob));
    assert_ne!(hers, his);
    assert_ne!(hers, owned_entry_id(compute_id(map, b"k2"), &alice));
    assert_eq!(
        owned_entry_id(hers, &alice),
        hers,
        "an owned id is its own slot"
    );
    assert_eq!(hers.as_bytes()[..12], his.as_bytes()[..12]);
    for id in [hers, his] {
        assert!(crate::collections::is_cell_owned_id(id));
        assert!(!crate::collections::is_cell_bound_id(id));
        assert!(!crate::collections::is_owned_id(id));
    }
    assert!(crate::collections::cell_owned_id_binds(hers, map, &alice));
    assert!(!crate::collections::cell_owned_id_binds(hers, map, &bob));
    assert!(!crate::collections::cell_owned_id_binds(
        hers,
        another_cells_map(),
        &alice
    ));
    assert!(!crate::collections::is_cell_bound_id(
        compute_collection_id(Some(hers), "nested")
    ));
}
