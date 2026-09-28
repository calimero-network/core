//! Two devices of one account writing one written-once key.
//!
//! Keys of an owned collection are per owner, so two accounts never write the
//! same entry (`owned_collisions.rs`). Two devices of one account do: both
//! write at the account's id for the key, and a device that has not seen the
//! other's write takes the key as fresh. So a node meets the second write as a
//! rewrite of an immutable entry.
//!
//! When apply kept whichever write reached it first, a node holding the phone's
//! value refused the laptop's and a node holding the laptop's skipped the
//! phone's as stale, and the two never agreed on a root hash again. Every node
//! must keep the same one of the two, whatever order they arrive in: the one
//! with the lower `(nonce, content hash)`.
//!
//! These tests replay each order on fresh stores, and a repair pushing each
//! node's leaf to the other, then play an account that does not own the entry
//! trying to take it with an earlier write.

use borsh::BorshSerialize;
use calimero_account::AccountId;
use serial_test::serial;
use sha2::{Digest, Sha256};

use crate::action::Action;
use crate::address::Id;
use crate::collections::{Root, UnorderedMap, WriteOnce};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{account_of_key, test_account};
use crate::tests::owned_rules::{act_as, apply, entry_bytes, key, later, signed};

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
    let data = entry_bytes(id, &ballot_key(), value);
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

/// What a node holds: the entries, the signed nonce of the write it kept, and
/// the hashes a peer compares against its own.
#[derive(Debug, PartialEq)]
struct Held {
    entries: Vec<(AccountId, String, String)>,
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
        entries: ballots.entries_with_owners().expect("entries"),
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

fn value_of(held: &Held) -> Option<&str> {
    match held.entries.as_slice() {
        [(_, _, value)] => Some(value.as_str()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Two devices, one key
// ---------------------------------------------------------------------------

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn two_devices_casting_one_key_converge_whatever_the_order() {
    for (phone_at, laptop_at) in [(AT, AT + 1), (AT + 1, AT), (AT, AT)] {
        let phone = (PHONE, "yes", phone_at);
        let laptop = (LAPTOP, "no", laptop_at);
        let phone_first = node_applying(&[phone, laptop]);
        let laptop_first = node_applying(&[laptop, phone]);
        assert_eq!(
            phone_first, laptop_first,
            "every node must keep the same ballot once it has seen both \
             (phone at {phone_at}, laptop at {laptop_at})"
        );
        assert_eq!(phone_first.entries.len(), 1, "one ballot per account");
    }
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn the_earliest_write_is_the_one_kept() {
    let phone = (PHONE, "yes", AT);
    let laptop = (LAPTOP, "no", AT + 1);
    for writes in [[phone, laptop], [laptop, phone]] {
        let held = node_applying(&writes);
        assert_eq!(value_of(&held), Some("yes"));
        assert_eq!(held.nonce, Some(AT));
    }
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn at_one_instant_the_lower_content_hash_is_kept() {
    let ballots = fresh_node();
    let id = entry_id(&ballots);
    let hash = |value: &str| -> [u8; 32] {
        Sha256::digest(entry_bytes(id, &ballot_key(), &value.to_owned())).into()
    };
    let expected = if hash("yes") < hash("no") {
        "yes"
    } else {
        "no"
    };

    let phone = (PHONE, "yes", AT);
    let laptop = (LAPTOP, "no", AT);
    for writes in [[phone, laptop], [laptop, phone]] {
        assert_eq!(value_of(&node_applying(&writes)), Some(expected));
    }
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
fn one_value_from_two_devices_settles_on_one_write() {
    // Byte-identical values at two instants hash alike, but a node keeping
    // the later one would judge a third write against a different nonce.
    let phone = (PHONE, "yes", AT);
    let laptop = (LAPTOP, "yes", AT + 2);
    let between = (PHONE, "no", AT + 1);
    let phone_first = node_applying(&[phone, laptop, between]);
    let laptop_first = node_applying(&[laptop, phone, between]);
    assert_eq!(phone_first, laptop_first);
    assert_eq!(phone_first.nonce, Some(AT));
    assert_eq!(value_of(&phone_first), Some("yes"));
}

#[test]
#[serial]
#[ignore = "fixed by the next commit: apply keeps whichever write arrived first"]
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
        let _ = apply(pushed.clone(), voter());
        (held(&ballots), leaf(entry_id(&ballots)))
    };
    let (phone_node, phone_node_leaf) = repaired(phone, &laptop_leaf);
    let (laptop_node, _) = repaired(laptop, &phone_leaf);
    assert_eq!(phone_node, laptop_node);
    assert_eq!(value_of(&phone_node), Some("no"));

    // The node that replaced its value ships a leaf whose signature is the
    // kept write's, so a third node verifies it.
    let ballots = fresh_node();
    let (device, value, at) = phone;
    apply(cast(&ballots, device, value, at), voter()).expect("a fresh key lands");
    apply(phone_node_leaf, voter()).expect("the repaired leaf verifies");
    assert_eq!(held(&ballots), laptop_node);
}

#[test]
#[serial]
fn a_redelivery_of_either_write_changes_nothing_once_settled() {
    let ballots = fresh_node();
    let phone = cast(&ballots, PHONE, "yes", AT);
    let laptop = cast(&ballots, LAPTOP, "no", AT + 1);
    apply(phone.clone(), voter()).expect("the first write lands");
    let settled = held(&ballots);
    assert!(matches!(
        apply(laptop, voter()),
        Err(StorageError::ActionNotAllowed(_))
    ));
    apply(phone, voter()).expect("a redelivery of the kept write is accepted");
    assert_eq!(held(&ballots), settled);
}

// ---------------------------------------------------------------------------
// Nobody but the owner, however early
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn a_later_write_from_another_device_is_refused() {
    let ballots = fresh_node();
    apply(cast(&ballots, PHONE, "yes", AT), voter()).expect("the first write lands");
    let settled = held(&ballots);
    assert!(matches!(
        apply(cast(&ballots, LAPTOP, "no", later()), voter()),
        Err(StorageError::ActionNotAllowed(_))
    ));
    assert_eq!(held(&ballots), settled);
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
        data: entry_bytes(id, &ballot_key(), &"no".to_owned()),
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
