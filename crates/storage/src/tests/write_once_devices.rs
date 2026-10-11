//! Two devices of one account writing one written-once key.
//!
//! Keys of an owned collection are per owner, so two accounts never write the
//! same entry (`owned_collisions.rs`). Two devices of one account do: both
//! write at the account's id for the key, and a device that has not seen the
//! other's write takes the key as fresh. So a node meets the second write as a
//! rewrite of an immutable entry.
//!
//! Two different values seal the entry: every node keeps the same two writes,
//! whatever order they arrive in, and reads the entry as conflicted. The same
//! value written twice settles on its earlier write.
//!
//! These tests replay each order on fresh stores, and a repair pushing each
//! node's leaf to the other, then play an account that does not own the entry
//! trying to take it with an earlier write.

use borsh::BorshSerialize;
use calimero_account::AccountId;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{
    Guarded, GuardedEntries, IndexedMap, Moderation, Once, OwnerOnce, Owning, Root, SortedMap,
    UnorderedMap, WriteOnce, Written,
};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{MainInterface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::map_entry_bytes;
use crate::tests::common::{account_of_key, conflict_record, test_account};
use crate::tests::owned_rules::{act_as, apply, conflicted, key, later, signed};
use crate::tests::policy_matrix::note;
use crate::written_once::conflict_parts;

/// Plain values, so two nodes holding the same entry hold the same bytes.
type Ballots = WriteOnce<UnorderedMap<String, String>>;

/// Writes carry fixed timestamps, so two nodes differ only by what they kept.
const AT: u64 = 1_700_000_000_000_000_000;

/// Two devices of the one account that owns every ballot here.
const PHONE: u8 = 0xD1;
const LAPTOP: u8 = 0xD2;

const MALLORY: u8 = 0xEE;

const RULES: EntryRules = EntryRules {
    immutable: true,
    moderators: None,
};

fn voter() -> AccountId {
    test_account(0x5A)
}

fn ballot_key() -> String {
    "k".to_owned()
}

/// One node's copy of the collection, with the same ids on every node.
fn fresh_node() -> Root<Ballots> {
    env::reset_for_testing();
    let _ = act_as(&key(0x01));
    Root::new(|| {
        let mut ballots = Ballots::new_with_field_name("ballots");
        ballots.reassign_deterministic_id("ballots");
        ballots
    })
}

fn ballots_id(ballots: &Root<Ballots>) -> Id {
    let inner: &UnorderedMap<String, String> = ballots;
    inner.id()
}

fn entry_id(ballots: &Root<Ballots>) -> Id {
    ballots.entry_id_of(&voter(), &ballot_key())
}

/// A signed `Add` of `(key, value)` at `id`, stamped `owner` and signed by
/// `signer`.
fn add_at<V: BorshSerialize>(
    parent: Id,
    id: Id,
    value: &V,
    owner: AccountId,
    signer: u8,
    at: u64,
) -> Action {
    let data = map_entry_bytes(id, &ballot_key(), value);
    signed(
        move |metadata| Action::Add {
            id,
            data,
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        owner,
        RULES,
        &key(signer),
        at,
    )
}

/// The voter's ballot `value`, cast from `device` at `at`.
fn cast(ballots: &Root<Ballots>, device: u8, value: &str, at: u64) -> Action {
    add_at(
        ballots_id(ballots),
        entry_id(ballots),
        &value.to_owned(),
        voter(),
        device,
        at,
    )
}

fn full_hash_of(id: Id) -> Option<[u8; 32]> {
    <Index<MainStorage>>::get_hashes_for(id)
        .expect("hashes")
        .map(|(full, _own)| full)
}

/// What a read of the voter's ballot returns.
#[derive(Debug, PartialEq)]
enum Read {
    Value(String),
    Absent,
    Conflicted,
}

fn read(ballots: &Root<Ballots>) -> Read {
    env::set_account_id(*voter().as_bytes());
    let read = ballots.get(&ballot_key());
    if conflicted(&read) {
        return Read::Conflicted;
    }
    match read.expect("a read of the ballot") {
        Some(value) => Read::Value(value),
        None => Read::Absent,
    }
}

/// What a node holds: the voter's ballot as a read returns it, the signed nonce
/// of the first write it kept, and the hashes a peer compares against its own.
#[derive(Debug, PartialEq)]
struct Held {
    read: Read,
    nonce: Option<u64>,
    collection_hash: Option<[u8; 32]>,
    root_hash: Option<[u8; 32]>,
}

fn held(ballots: &Root<Ballots>) -> Held {
    let nonce = <Index<MainStorage>>::get_metadata(entry_id(ballots))
        .expect("metadata")
        .and_then(|metadata| match metadata.storage_type {
            StorageType::User {
                signature_data: Some(sig),
                ..
            } => Some(sig.nonce),
            _ => None,
        });
    Held {
        read: read(ballots),
        nonce,
        collection_hash: full_hash_of(ballots_id(ballots)),
        root_hash: full_hash_of(Id::root()),
    }
}

/// Apply `writes` in order on a fresh node, each signed by a device of the
/// voter, and return what it holds. A write that loses is refused.
fn node_applying(writes: &[(u8, &str, u64)]) -> Held {
    let ballots = fresh_node();
    for &(device, value, at) in writes {
        let _ = apply(cast(&ballots, device, value, at), voter());
    }
    held(&ballots)
}

/// The stored entry as a repair ships it: its bytes and its index metadata.
fn leaf(id: Id) -> Action {
    Action::Update {
        id,
        data: MainStorage::storage_read(Key::Entry(id)).expect("stored"),
        ancestors: vec![],
        metadata: <Index<MainStorage>>::get_metadata(id)
            .expect("metadata")
            .expect("present"),
    }
}

/// Apply a peer's leaf as a repair does: a sealed entry's record as the writes
/// it holds, each on its own.
fn repair(leaf: &Action) {
    let Action::Update {
        id, data, metadata, ..
    } = leaf
    else {
        panic!("a leaf is an update");
    };
    let Some(writes) = conflict_parts(&metadata.storage_type, data) else {
        let _ = apply(leaf.clone(), voter());
        return;
    };
    for (data, storage_type) in writes {
        let write = Action::Update {
            id: *id,
            data,
            ancestors: vec![],
            metadata: Metadata {
                storage_type,
                ..metadata.clone()
            },
        };
        let _ = apply(write, voter());
    }
}

/// A node once the voter's phone and laptop cast different ballots.
fn sealed_node() -> Root<Ballots> {
    let ballots = fresh_node();
    for (device, value, at) in [(PHONE, "yes", AT), (LAPTOP, "no", AT + 1)] {
        apply(cast(&ballots, device, value, at), voter()).expect("each write lands");
    }
    ballots
}

/// A node's leaf once the voter's phone and laptop cast different ballots.
fn sealed_leaf() -> Action {
    leaf(entry_id(&sealed_node()))
}

// ---------------------------------------------------------------------------
// Two devices, one key
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn two_devices_casting_one_key_converge_whatever_the_order() {
    for (phone_at, laptop_at) in [(AT, AT + 1), (AT + 1, AT), (AT, AT)] {
        let phone = (PHONE, "yes", phone_at);
        let laptop = (LAPTOP, "no", laptop_at);
        let phone_first = node_applying(&[phone, laptop]);
        let laptop_first = node_applying(&[laptop, phone]);
        assert_eq!(
            phone_first, laptop_first,
            "every node must hold the same entry once it has seen both \
             (phone at {phone_at}, laptop at {laptop_at})"
        );
        assert_eq!(phone_first.read, Read::Conflicted, "two values seal it");
    }
}

#[test]
#[serial]
fn two_values_from_two_devices_seal_it_from_the_earliest_write() {
    let phone = (PHONE, "yes", AT);
    let laptop = (LAPTOP, "no", AT + 1);
    for writes in [[phone, laptop], [laptop, phone]] {
        let held = node_applying(&writes);
        assert_eq!(held.read, Read::Conflicted);
        assert_eq!(held.nonce, Some(AT));
    }
}

#[test]
#[serial]
fn one_value_from_two_devices_settles_on_one_write() {
    // Byte-identical values at two instants hash alike, but a node keeping
    // the later one would seal on a different pair once a third write lands.
    let phone = (PHONE, "yes", AT);
    let laptop = (LAPTOP, "yes", AT + 2);
    for writes in [[phone, laptop], [laptop, phone]] {
        let held = node_applying(&writes);
        assert_eq!(held.read, Read::Value("yes".to_owned()));
        assert_eq!(held.nonce, Some(AT));
    }

    let between = (PHONE, "no", AT + 1);
    let phone_first = node_applying(&[phone, laptop, between]);
    let laptop_first = node_applying(&[laptop, phone, between]);
    assert_eq!(phone_first, laptop_first);
    assert_eq!(phone_first.read, Read::Conflicted);
    assert_eq!(phone_first.nonce, Some(AT));
}

#[test]
#[serial]
fn nodes_sealed_by_different_pairs_converge_after_an_exchange() {
    let first = (PHONE, "a", AT);
    let second = (LAPTOP, "b", AT + 1);
    let third = (PHONE, "c", AT + 2);
    let node = |writes: &[(u8, &str, u64)]| {
        let ballots = fresh_node();
        for &(device, value, at) in writes {
            let _ = apply(cast(&ballots, device, value, at), voter());
        }
        ballots
    };
    let early_leaf = leaf(entry_id(&node(&[first, second])));
    let late_leaf = leaf(entry_id(&node(&[second, third])));

    // Each node takes the other's leaf, as HashComparison hands it over.
    let exchanged = |own: &[(u8, &str, u64)], pushed: &Action| {
        let ballots = node(own);
        repair(pushed);
        let id = entry_id(&ballots);
        (held(&ballots), MainStorage::storage_read(Key::Entry(id)))
    };
    let (early, early_bytes) = exchanged(&[first, second], &late_leaf);
    let (late, late_bytes) = exchanged(&[second, third], &early_leaf);
    assert_eq!(
        early_bytes, late_bytes,
        "both nodes must store the same bytes"
    );
    assert_eq!(early, late);
    assert_eq!(early.read, Read::Conflicted);
    assert_eq!(early.nonce, Some(AT));
}

#[test]
#[serial]
fn a_repair_pushing_either_node_s_leaf_settles_both() {
    let phone = (PHONE, "yes", AT + 1);
    let laptop = (LAPTOP, "no", AT);

    // Each node's own leaf, before either has seen the other's write.
    let leaf_holding = |write: (u8, &str, u64)| {
        let ballots = fresh_node();
        let (device, value, at) = write;
        apply(cast(&ballots, device, value, at), voter()).expect("a fresh key lands");
        leaf(entry_id(&ballots))
    };
    let phone_leaf = leaf_holding(phone);
    let laptop_leaf = leaf_holding(laptop);

    // Each node takes the other's leaf, as HashComparison hands it over.
    let repaired = |own: (u8, &str, u64), pushed: &Action| {
        let ballots = fresh_node();
        let (device, value, at) = own;
        apply(cast(&ballots, device, value, at), voter()).expect("a fresh key lands");
        repair(pushed);
        (held(&ballots), leaf(entry_id(&ballots)))
    };
    let (phone_node, phone_node_leaf) = repaired(phone, &laptop_leaf);
    let (laptop_node, _) = repaired(laptop, &phone_leaf);
    assert_eq!(phone_node, laptop_node);
    assert_eq!(phone_node.read, Read::Conflicted);

    // The sealed node's leaf carries both writes, so a third node holding one
    // of them verifies it and seals the same way.
    let ballots = fresh_node();
    let (device, value, at) = phone;
    apply(cast(&ballots, device, value, at), voter()).expect("a fresh key lands");
    repair(&phone_node_leaf);
    assert_eq!(held(&ballots), laptop_node);
}

#[test]
#[serial]
fn a_redelivery_of_either_write_changes_nothing_once_settled() {
    let ballots = fresh_node();
    let phone = cast(&ballots, PHONE, "yes", AT);
    let laptop = cast(&ballots, LAPTOP, "no", AT + 1);
    apply(phone.clone(), voter()).expect("the first write lands");
    apply(laptop.clone(), voter()).expect("the second value seals it");
    let settled = held(&ballots);
    apply(phone, voter()).expect("a redelivery of either write is accepted");
    apply(laptop, voter()).expect("a redelivery of either write is accepted");
    assert_eq!(held(&ballots), settled);
}

#[test]
#[serial]
fn a_sealed_entry_stays_in_every_owner_listing() {
    let ballots = sealed_node();
    let sealed = Written::<String>::Conflicted;
    assert_eq!(
        ballots.entries_with_owners().expect("entries"),
        vec![(voter(), ballot_key(), sealed.clone())]
    );
    assert_eq!(
        ballots.entries_by(&voter()).expect("by"),
        vec![(ballot_key(), sealed.clone())]
    );
    assert_eq!(
        ballots.entries_at(&ballot_key()).expect("at"),
        vec![(voter(), sealed)]
    );
}

/// A collection of `C` under policy `P` holding the voter's entry at `k`,
/// sealed by the phone's and the laptop's `values`.
fn sealed_collection<C, P>(values: [C::Value; 2]) -> Root<Guarded<C, P>>
where
    C: GuardedEntries<Key = String>,
    P: Owning,
{
    env::reset_for_testing();
    let _ = act_as(&key(0x01));
    let sealed = Root::new(|| {
        let mut collection = Guarded::<C, P>::new_with_field_name("sealed");
        collection.reassign_deterministic_id("sealed");
        collection
    });
    let parent = (**sealed).id();
    let id = sealed.entry_id_of(&voter(), &ballot_key());
    let rules = sealed.entry_rules();
    for ((device, at), value) in [(PHONE, AT), (LAPTOP, AT + 1)].into_iter().zip(values) {
        let data = map_entry_bytes(id, &ballot_key(), &value);
        let write = signed(
            move |metadata| Action::Add {
                id,
                data,
                ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
                metadata,
            },
            voter(),
            rules,
            &key(device),
            at,
        );
        apply(write, voter()).expect("each write lands");
    }
    sealed
}

#[test]
#[serial]
fn a_sealed_entry_stays_in_every_listing_of_the_collection() {
    let values = || ["yes".to_owned(), "no".to_owned()];

    let map = sealed_collection::<UnorderedMap<String, String>, OwnerOnce>(values());
    assert_eq!(
        map.entries().expect("entries").count(),
        1,
        "UnorderedMap::entries"
    );

    let moderated = sealed_collection::<UnorderedMap<String, String>, Moderation<Once>>(values());
    assert_eq!(
        moderated.entries().expect("entries").count(),
        1,
        "ModeratedOnce::entries"
    );

    let sorted = sealed_collection::<SortedMap<String, String>, OwnerOnce>(values());
    assert_eq!(
        sorted.entries().expect("entries").count(),
        1,
        "SortedMap::entries"
    );
    assert_eq!(sorted.keys().expect("keys").count(), 1, "SortedMap::keys");
    assert_eq!(
        sorted.values().expect("values").count(),
        1,
        "SortedMap::values"
    );
    assert_eq!(
        sorted.range(..).expect("range").count(),
        1,
        "SortedMap::range"
    );
    assert_eq!(
        sorted.prefix(b"k").expect("prefix").count(),
        1,
        "SortedMap::prefix"
    );
    assert_eq!(
        sorted.page(0, 10).expect("page").len(),
        1,
        "SortedMap::page"
    );
    assert!(sorted.first().expect("first").is_some(), "SortedMap::first");
    assert!(sorted.last().expect("last").is_some(), "SortedMap::last");

    let indexed = sealed_collection::<
        IndexedMap<String, crate::tests::policy_matrix::Note>,
        OwnerOnce,
    >([note("t", "yes"), note("t", "no")]);
    assert_eq!(
        indexed.entries().expect("entries").count(),
        1,
        "IndexedMap::entries"
    );
}

#[test]
#[serial]
fn a_written_once_value_too_large_for_a_record_of_two_is_refused() {
    let ballots = fresh_node();
    let large = "x".repeat(600 * 1024);
    assert!(matches!(
        apply(cast(&ballots, PHONE, &large, AT), voter()),
        Err(StorageError::ActionNotAllowed(_))
    ));
}

#[test]
#[serial]
fn a_sealed_entry_s_record_is_never_applied_as_one_write() {
    let record = sealed_leaf();
    let _node = fresh_node();
    assert!(matches!(
        apply(record, voter()),
        Err(StorageError::ActionNotAllowed(_))
    ));
}

#[test]
#[serial]
fn a_snapshot_holds_a_record_only_if_each_of_its_writes_verifies() {
    let Action::Update {
        id, data, metadata, ..
    } = sealed_leaf()
    else {
        panic!("a leaf is an update");
    };
    let parent = ballots_id(&fresh_node());
    MainInterface::verify_snapshot_entity_signature(id, Some(parent), &data, &metadata)
        .expect("a record of two genuine writes verifies");

    let [(first, first_type), (_, second_type)] =
        conflict_parts(&metadata.storage_type, &data).expect("a record");
    let signature = |stamp: &StorageType| *stamp.signature_data().expect("signed");
    let other = map_entry_bytes(id, &ballot_key(), &"maybe".to_owned());
    let (forged, stamp) = conflict_record(
        &metadata.storage_type,
        [
            (first, signature(&first_type)),
            (other, signature(&second_type)),
        ],
    );
    let metadata = Metadata {
        storage_type: stamp,
        ..metadata
    };
    assert!(matches!(
        MainInterface::verify_snapshot_entity_signature(id, Some(parent), &forged, &metadata),
        Err(StorageError::InvalidSignature)
    ));
}

// ---------------------------------------------------------------------------
// Nobody but the owner, however early
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn a_later_write_from_another_device_seals_it() {
    let ballots = fresh_node();
    apply(cast(&ballots, PHONE, "yes", AT), voter()).expect("the first write lands");
    apply(cast(&ballots, LAPTOP, "no", later()), voter()).expect("a second value seals it");
    let held = held(&ballots);
    assert_eq!(held.read, Read::Conflicted);
    assert_eq!(held.nonce, Some(AT));
}

#[test]
#[serial]
fn no_other_account_replaces_it_with_an_earlier_write() {
    let ballots = fresh_node();
    apply(cast(&ballots, PHONE, "yes", AT + 10), voter()).expect("the voter's write lands");
    let settled = held(&ballots);
    let mallory = account_of_key(&key(MALLORY));
    let (parent, id) = (ballots_id(&ballots), entry_id(&ballots));
    let earlier = AT;

    // Claiming the voter's ownership, signed by a key that does not speak for
    // the voter.
    let forged = add_at(parent, id, &"no".to_owned(), voter(), MALLORY, earlier);
    assert!(apply(forged, mallory).is_err());

    // Owned by Mallory, at the voter's id.
    let squat = add_at(parent, id, &"no".to_owned(), mallory, MALLORY, earlier);
    assert!(apply(squat, mallory).is_err());

    // Unsigned, at the voter's id.
    let unsigned = Action::Update {
        id,
        data: map_entry_bytes(id, &ballot_key(), &"no".to_owned()),
        ancestors: vec![],
        metadata: Metadata {
            created_at: earlier,
            updated_at: earlier.into(),
            storage_type: StorageType::Public,
            ..Metadata::default()
        },
    };
    assert!(apply(unsigned, mallory).is_err());

    assert_eq!(held(&ballots), settled);
}
