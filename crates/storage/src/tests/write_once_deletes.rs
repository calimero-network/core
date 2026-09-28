//! Deleting a written-once entry removes its owner's key for good.
//!
//! A `ModeratedOnce` entry is written once and a moderator may remove it. Two
//! devices of the owner can write the key concurrently (`write_once_devices.rs`),
//! and the moderator's delete can land between their timestamps, before either
//! write, or after both. Judging the delete by last-writer-wins against
//! whichever write a node held split the nodes: one node's delete lost to the
//! later write it held, another's won against the earlier one, and a node
//! handed the delete before any write could not place it at all.
//!
//! So a delete of a written-once entry is terminal: once a node holds it, no
//! write to that owner's key lands again, earlier or later, and a node given
//! the delete first keeps it for the writes that follow. These tests replay
//! every interleaving of the two writes and the delete, then play a repair, a
//! backdated write, a local re-insert and a delete by someone who may not.

use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{ModeratedOnce, Root, UnorderedMap};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{account_of_key, test_account};
use crate::tests::owned_rules::{act_as, apply, delete, entry_bytes, key, refused, signed};

/// Plain values, so two nodes holding the same entry hold the same bytes.
type Chat = ModeratedOnce<UnorderedMap<String, String>>;

/// Writes carry fixed timestamps, so two nodes differ only by what they kept.
const AT: u64 = 1_700_000_000_000_000_000;

/// Two devices of the one account that owns every message here.
const PHONE: u8 = 0xD1;
const LAPTOP: u8 = 0xD2;

const MODERATOR: u8 = 0x40;
const MALLORY: u8 = 0xEE;

fn author() -> AccountId {
    test_account(0x5A)
}

fn message_key() -> String {
    "m".to_owned()
}

/// One node's copy of the chat, moderated by `MODERATOR`, with the same ids on
/// every node.
fn fresh_node() -> Root<Chat> {
    env::reset_for_testing();
    let _ = act_as(&key(MODERATOR));
    Root::new(|| {
        let mut chat = Chat::new_with_field_name("chat");
        chat.reassign_deterministic_id("chat");
        chat
    })
}

fn chat_id(chat: &Root<Chat>) -> Id {
    let inner: &UnorderedMap<String, String> = chat;
    inner.id()
}

fn entry_id(chat: &Root<Chat>) -> Id {
    chat.entry_id_of(&author(), &message_key())
}

/// The rules every entry of the chat carries: written once, and moderated by
/// the chat's moderators.
fn rules(chat: &Root<Chat>) -> EntryRules {
    let rules = chat.entry_rules();
    assert!(rules.immutable && rules.moderators.is_some());
    rules
}

/// The author's message `value`, sent from `device` at `at`.
fn send(chat: &Root<Chat>, device: u8, value: &str, at: u64) -> Action {
    let (parent, id) = (chat_id(chat), entry_id(chat));
    let data = entry_bytes(id, &message_key(), &value.to_owned());
    signed(
        move |metadata| Action::Add {
            id,
            data,
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        author(),
        rules(chat),
        &key(device),
        at,
    )
}

/// `signer`'s delete of the author's message at `at`.
fn removal(chat: &Root<Chat>, signer: &SigningKey, at: u64) -> Action {
    signed(
        delete(entry_id(chat), at),
        author(),
        rules(chat),
        signer,
        at,
    )
}

/// One step of an interleaving: which write or delete, and when it was made.
#[derive(Clone, Copy, Debug)]
enum Step {
    Phone(u64),
    Laptop(u64),
    Delete(u64),
}

impl Step {
    fn action(self, chat: &Root<Chat>) -> (Action, AccountId) {
        match self {
            Self::Phone(at) => (send(chat, PHONE, "hello", at), author()),
            Self::Laptop(at) => (send(chat, LAPTOP, "hi", at), author()),
            Self::Delete(at) => (
                removal(chat, &key(MODERATOR), at),
                account_of_key(&key(MODERATOR)),
            ),
        }
    }
}

fn full_hash_of(id: Id) -> Option<[u8; 32]> {
    <Index<MainStorage>>::get_hashes_for(id)
        .expect("hashes")
        .map(|(full, _own)| full)
}

/// What a node holds: the entries, whether it keeps the author's key deleted,
/// and the hash a peer compares against its own.
///
/// The collection's hash, not the root's: the root also holds the moderators'
/// cell, whose register each fresh node stamps with its own clock.
#[derive(Debug, PartialEq)]
struct Held {
    entries: Vec<(AccountId, String, String)>,
    deleted: bool,
    collection_hash: Option<[u8; 32]>,
}

fn held(chat: &Root<Chat>) -> Held {
    let id = entry_id(chat);
    Held {
        entries: chat.entries_with_owners().expect("entries"),
        deleted: <Index<MainStorage>>::is_deleted(id).expect("index")
            || <Index<MainStorage>>::is_sealed(id, &rules(chat)).expect("seal"),
        collection_hash: full_hash_of(chat_id(chat)),
    }
}

/// Apply `steps` in order on a fresh node and return what it holds. A step
/// that loses is refused.
fn node_applying(steps: &[Step]) -> Held {
    let chat = fresh_node();
    for step in steps {
        let (action, signer) = step.action(&chat);
        let _ = apply(action, signer);
    }
    held(&chat)
}

/// Every order of `steps`.
fn orders(steps: [Step; 3]) -> Vec<[Step; 3]> {
    let [a, b, c] = steps;
    vec![
        [a, b, c],
        [a, c, b],
        [b, a, c],
        [b, c, a],
        [c, a, b],
        [c, b, a],
    ]
}

/// A node that took the delete but none of the writes: the reference every
/// interleaving must reach.
fn deleted_everywhere() -> Held {
    let chat = fresh_node();
    let held = held(&chat);
    Held {
        deleted: true,
        ..held
    }
}

// ---------------------------------------------------------------------------
// Two devices and a moderator
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn a_delete_among_two_devices_writes_converges_on_deleted_in_every_order() {
    let phone = Step::Phone(AT);
    let laptop = Step::Laptop(AT + 2);
    let expected = deleted_everywhere();
    for deleted_at in [AT - 1, AT + 1, AT + 3] {
        for steps in orders([phone, laptop, Step::Delete(deleted_at)]) {
            assert_eq!(
                node_applying(&steps),
                expected,
                "every node must end with the author's key deleted ({steps:?})"
            );
        }
    }
}

#[test]
#[serial]
fn a_backdated_write_after_the_delete_is_refused() {
    let chat = fresh_node();
    let (write, author) = Step::Phone(AT + 10).action(&chat);
    apply(write, author).expect("the author's message lands");
    let (removal, moderator) = Step::Delete(AT + 20).action(&chat);
    apply(removal, moderator).expect("the moderator's delete applies");

    for at in [AT, AT + 15, AT + 30] {
        assert!(matches!(
            apply(send(&chat, LAPTOP, "again", at), author),
            Err(StorageError::ActionNotAllowed(_))
        ));
    }
    assert_eq!(held(&chat), deleted_everywhere());
}

#[test]
#[serial]
fn a_repair_pushing_a_live_leaf_does_not_resurrect_it() {
    // A node that never saw the delete ships its live entry, as
    // HashComparison hands it over.
    let chat = fresh_node();
    let (write, author) = Step::Phone(AT).action(&chat);
    apply(write, author).expect("the author's message lands");
    let id = entry_id(&chat);
    let leaf = Action::Update {
        id,
        data: MainStorage::storage_read(Key::Entry(id)).expect("stored"),
        ancestors: vec![],
        metadata: <Index<MainStorage>>::get_metadata(id)
            .expect("metadata")
            .expect("present"),
    };

    let chat = fresh_node();
    for step in [Step::Phone(AT), Step::Delete(AT + 1)] {
        let (action, signer) = step.action(&chat);
        apply(action, signer).expect("applies");
    }
    assert!(apply(leaf, author).is_err());
    assert_eq!(held(&chat), deleted_everywhere());
}

#[test]
#[serial]
fn a_node_given_the_delete_first_keeps_it() {
    let chat = fresh_node();
    let (removal, moderator) = Step::Delete(AT + 5).action(&chat);
    apply(removal, moderator).expect("a moderator's delete of an unseen entry is kept");
    for step in [Step::Phone(AT), Step::Laptop(AT + 10)] {
        let (write, author) = step.action(&chat);
        assert!(matches!(
            apply(write, author),
            Err(StorageError::ActionNotAllowed(_))
        ));
    }
    assert_eq!(held(&chat), deleted_everywhere());
}

#[test]
#[serial]
fn a_redelivered_delete_is_accepted_and_changes_nothing() {
    let chat = fresh_node();
    for step in [Step::Phone(AT), Step::Delete(AT + 1)] {
        let (action, signer) = step.action(&chat);
        apply(action, signer).expect("applies");
    }
    let settled = held(&chat);
    for at in [AT + 1, AT - 1] {
        let (again, moderator) = Step::Delete(at).action(&chat);
        apply(again, moderator).expect("a delete of a deleted entry is a no-op");
    }
    assert_eq!(held(&chat), settled);
}

// ---------------------------------------------------------------------------
// Locally
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn the_owner_cannot_write_a_deleted_key_again() {
    env::reset_for_testing();
    let moderator = key(MODERATOR);
    let _ = act_as(&moderator);
    let mut chat = Root::new(Chat::new);
    let alice = key(0xA1);
    let _ = act_as(&alice);
    chat.insert(message_key(), "hello".to_owned())
        .expect("alice posts");

    let _ = act_as(&moderator);
    assert!(chat
        .remove_by(&account_of_key(&alice), &message_key())
        .expect("the moderator removes it")
        .is_some());

    let _ = act_as(&alice);
    assert!(refused(chat.insert(message_key(), "again".to_owned())));
    assert!(chat.get(&message_key()).expect("get").is_none());
}

// ---------------------------------------------------------------------------
// Only who may delete today
// ---------------------------------------------------------------------------

#[test]
#[serial]
fn no_one_but_a_moderator_deletes_it_even_first() {
    for signer in [key(MALLORY), key(PHONE)] {
        let chat = fresh_node();
        let account = if signer == key(PHONE) {
            author()
        } else {
            account_of_key(&signer)
        };
        assert!(apply(removal(&chat, &signer, AT + 5), account).is_err());
        let (write, author) = Step::Phone(AT).action(&chat);
        apply(write, author).expect("a refused delete leaves the key free");
        assert!(apply(removal(&chat, &signer, AT + 5), account).is_err());
        let held = held(&chat);
        assert!(!held.deleted);
        assert_eq!(held.entries.len(), 1);
    }
}

#[test]
#[serial]
fn only_an_immutable_entry_s_delete_is_terminal() {
    // An editable moderated entry keeps last-writer-wins: a later write by its
    // owner still brings it back.
    type Board = crate::collections::Moderated<UnorderedMap<String, String>>;

    env::reset_for_testing();
    let moderator = key(MODERATOR);
    let _ = act_as(&moderator);
    let mut board = Root::new(Board::new);
    let alice = key(0xA1);
    let _ = act_as(&alice);
    board
        .insert(message_key(), "hello".to_owned())
        .expect("alice posts");
    let _ = act_as(&moderator);
    let _ = board
        .remove_by(&account_of_key(&alice), &message_key())
        .expect("the moderator removes it");
    let _ = act_as(&alice);
    board
        .insert(message_key(), "again".to_owned())
        .expect("an editable key is free again");
    assert_eq!(
        board.get(&message_key()).expect("get"),
        Some("again".to_owned())
    );
}

/// The record a node keeps of the delete names the owner and the rules, so
/// tombstone GC can tell it must last.
#[test]
#[serial]
fn the_record_of_the_delete_is_marked_as_lasting() {
    let chat = fresh_node();
    for step in [Step::Phone(AT), Step::Delete(AT + 1)] {
        let (action, signer) = step.action(&chat);
        apply(action, signer).expect("applies");
    }
    let index = <Index<MainStorage>>::get_index(entry_id(&chat))
        .expect("index")
        .expect("tombstone");
    assert!(index.is_terminal_tombstone());
    assert!(matches!(
        index.metadata.storage_type,
        StorageType::User { owner, .. } if owner == author()
    ));
}

#[test]
#[serial]
fn a_repair_carrying_the_tombstone_deletes_it_where_it_was_missed() {
    // A node that took the delete ships its tombstone as HashComparison does
    // (`deleted_children`): the id, `deleted_at` and the stored stamp.
    let chat = fresh_node();
    for step in [Step::Phone(AT), Step::Delete(AT + 1)] {
        let (action, signer) = step.action(&chat);
        apply(action, signer).expect("applies");
    }
    let id = entry_id(&chat);
    let tombstone = <Index<MainStorage>>::get_index(id)
        .expect("index")
        .expect("tombstone");
    let shipped = Action::DeleteRef {
        id,
        deleted_at: tombstone.deleted_at.expect("deleted"),
        metadata: tombstone.metadata,
    };

    // A node that missed it holds the later device's write.
    let chat = fresh_node();
    let (write, author) = Step::Laptop(AT + 2).action(&chat);
    apply(write, author).expect("the laptop's message lands");
    apply(shipped, account_of_key(&key(MODERATOR))).expect("the shipped delete verifies");
    assert_eq!(held(&chat), deleted_everywhere());
}

#[test]
#[serial]
fn a_seal_under_rules_naming_other_moderators_does_not_hold_the_key() {
    // Mallory signs a delete of the author's key under rules naming a moderator
    // set of her own, before the author writes. It can only ever match a write
    // under those rules, never the author's.
    let chat = fresh_node();
    let forged_rules = EntryRules {
        immutable: true,
        moderators: Some(Id::new([0x77; 32])),
    };
    let forged = signed(
        delete(entry_id(&chat), AT + 5),
        author(),
        forged_rules,
        &key(MALLORY),
        AT + 5,
    );
    let _ = apply(forged, account_of_key(&key(MALLORY)));
    let (write, author) = Step::Phone(AT).action(&chat);
    apply(write, author).expect("the author's write under the chat's rules lands");
    assert_eq!(value_count(&chat), 1);
}

fn value_count(chat: &Root<Chat>) -> usize {
    chat.entries_with_owners().expect("entries").len()
}
