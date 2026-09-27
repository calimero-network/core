//! Owned entries under rules: written once, and moderated.
//!
//! The rules ride in the owner stamp, are signed with the entry, and are fixed
//! at creation. These tests build state through the collections, then play
//! peers through `Interface::apply_action`: an owner trying to rewrite what it
//! wrote once, a stranger and a revoked moderator trying to delete, an author
//! trying to relax its entry's rules, and a relay tampering with them in
//! transit.

use borsh::{to_vec, BorshSerialize};
use calimero_account::AccountId;
use ed25519_dalek::SigningKey;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::{
    Authored, LwwRegister, Moderated, ModeratedOnce, Root, UnorderedMap, WriteOnce,
};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, SignatureData, StorageType};
use crate::env;
use crate::index::Index;
use crate::interface::{Interface, StorageError};
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::{account_of_key, apply_ctx_for, pubkey_of, sign_action};

type MainInterface = Interface<MainStorage>;
type Messages = WriteOnce<UnorderedMap<String, LwwRegister<String>>>;
type Board = Moderated<UnorderedMap<String, LwwRegister<String>>>;
type Chat = ModeratedOnce<UnorderedMap<String, LwwRegister<String>>>;

pub(super) fn key(seed: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed; 32])
}

/// Act as the account `sk` speaks for.
pub(super) fn act_as(sk: &SigningKey) -> AccountId {
    let account = account_of_key(sk);
    env::set_account_id(*account.as_bytes());
    account
}

fn text(value: &str) -> LwwRegister<String> {
    LwwRegister::new(value.to_owned())
}

fn stored_metadata(id: Id) -> Metadata {
    <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .expect("present")
}

pub(super) fn rules_of(id: Id) -> EntryRules {
    match stored_metadata(id).storage_type {
        StorageType::User { rules, .. } => rules,
        other => panic!("expected an owned entry, got {other:?}"),
    }
}

pub(super) fn refused<T: core::fmt::Debug>(
    result: Result<T, crate::collections::StoreError>,
) -> bool {
    matches!(
        result,
        Err(crate::collections::StoreError::StorageError(
            StorageError::ActionNotAllowed(_)
        ))
    )
}

/// The bytes a map entry is stored as: the `(value, key)` item, then its id.
fn entry_bytes<K: BorshSerialize, V: BorshSerialize>(id: Id, key: &K, value: &V) -> Vec<u8> {
    to_vec(&((value, key), id)).expect("serialize entry")
}

/// A signed owner-stamped action, as a peer's node would produce it.
pub(super) fn signed(
    action_of: impl FnOnce(Metadata) -> Action,
    owner: AccountId,
    rules: EntryRules,
    signer: &SigningKey,
    nonce: u64,
) -> Action {
    let metadata = Metadata {
        created_at: nonce,
        updated_at: nonce.into(),
        storage_type: StorageType::User {
            owner,
            rules,
            signature_data: Some(SignatureData {
                signature: [0; 64],
                nonce,
                signer: Some(pubkey_of(signer)),
            }),
        },
        ..Metadata::default()
    };
    let mut action = action_of(metadata);
    let signature = sign_action(&action, signer);
    let metadata = match &mut action {
        Action::Add { metadata, .. }
        | Action::Update { metadata, .. }
        | Action::DeleteRef { metadata, .. } => metadata,
    };
    if let StorageType::User {
        signature_data: Some(sig),
        ..
    } = &mut metadata.storage_type
    {
        sig.signature = signature;
    }
    action
}

pub(super) fn later() -> u64 {
    env::time_now() + 1_000_000_000
}

pub(super) fn update(id: Id, parent: Id, data: Vec<u8>) -> impl FnOnce(Metadata) -> Action {
    move |metadata| Action::Update {
        id,
        data,
        ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
        metadata,
    }
}

pub(super) fn delete(id: Id, at: u64) -> impl FnOnce(Metadata) -> Action {
    move |metadata| Action::DeleteRef {
        id,
        deleted_at: at,
        metadata,
    }
}

pub(super) fn apply(action: Action, signer_account: AccountId) -> Result<(), StorageError> {
    MainInterface::apply_action(action, &apply_ctx_for(signer_account))
}

pub(super) fn is_gone(id: Id) -> bool {
    MainStorage::storage_read(Key::Entry(id)).is_none()
        || <Index<MainStorage>>::get_index(id)
            .ok()
            .flatten()
            .is_some_and(|index| index.deleted_at.is_some())
}

// ---------------------------------------------------------------------------
// Written once
// ---------------------------------------------------------------------------

fn alices_message() -> (Root<Messages>, SigningKey, Id) {
    env::reset_for_testing();
    let alice = key(0xA1);
    let _ = act_as(&alice);
    let mut messages = Root::new(Messages::new);
    messages
        .insert("m1".to_owned(), text("hello"))
        .expect("insert");
    let id = messages.entry_id(&"m1".to_owned());
    (messages, alice, id)
}

#[test]
#[serial]
fn a_write_once_entry_is_owned_and_immutable() {
    let (messages, alice, id) = alices_message();
    assert_eq!(
        rules_of(id),
        EntryRules {
            immutable: true,
            moderators: None
        }
    );
    assert_eq!(
        messages.owner_of(&"m1".to_owned()).expect("owner"),
        Some(account_of_key(&alice))
    );
    assert_eq!(
        messages
            .get(&"m1".to_owned())
            .expect("get")
            .map(|m| m.get().clone()),
        Some("hello".to_owned())
    );
}

#[test]
#[serial]
fn a_write_once_key_cannot_be_taken_again() {
    let (mut messages, _alice, _id) = alices_message();
    assert!(refused(messages.insert("m1".to_owned(), text("again"))));
}

#[test]
#[serial]
fn no_node_accepts_a_rewrite_even_signed_by_the_owner() {
    let (messages, alice, id) = alices_message();
    let parent = (**messages).id();
    let rewrite = signed(
        update(
            id,
            parent,
            entry_bytes(id, &"m1".to_owned(), &text("edited")),
        ),
        account_of_key(&alice),
        rules_of(id),
        &alice,
        later(),
    );
    assert!(matches!(
        apply(rewrite, account_of_key(&alice)),
        Err(StorageError::ActionNotAllowed(_))
    ));
    assert_eq!(
        messages
            .get(&"m1".to_owned())
            .expect("get")
            .map(|m| m.get().clone()),
        Some("hello".to_owned())
    );
}

#[test]
#[serial]
fn a_redelivery_of_the_same_bytes_is_accepted() {
    let (messages, alice, id) = alices_message();
    let parent = (**messages).id();
    let stored = MainStorage::storage_read(Key::Entry(id)).expect("stored");
    let again = signed(
        update(id, parent, stored),
        account_of_key(&alice),
        rules_of(id),
        &alice,
        later(),
    );
    apply(again, account_of_key(&alice)).expect("sync redelivers what it already has");
}

#[test]
#[serial]
fn no_node_accepts_a_delete_even_signed_by_the_owner() {
    let (_messages, alice, id) = alices_message();
    let at = later();
    let removal = signed(
        delete(id, at),
        account_of_key(&alice),
        rules_of(id),
        &alice,
        at,
    );
    assert!(apply(removal, account_of_key(&alice)).is_err());
    assert!(!is_gone(id));
}

#[test]
#[serial]
fn an_owner_cannot_relax_the_rules_it_wrote_under() {
    let (messages, alice, id) = alices_message();
    let parent = (**messages).id();
    let relaxed = signed(
        update(
            id,
            parent,
            entry_bytes(id, &"m1".to_owned(), &text("edited")),
        ),
        account_of_key(&alice),
        EntryRules::OWNED,
        &alice,
        later(),
    );
    assert!(matches!(
        apply(relaxed, account_of_key(&alice)),
        Err(StorageError::ActionNotAllowed(_))
    ));
    let at = later();
    let relaxed_delete = signed(
        delete(id, at),
        account_of_key(&alice),
        EntryRules::OWNED,
        &alice,
        at,
    );
    assert!(apply(relaxed_delete, account_of_key(&alice)).is_err());
    assert!(!is_gone(id));
}

#[test]
#[serial]
fn rules_tampered_with_in_transit_fail_the_signature() {
    let (messages, alice, id) = alices_message();
    let parent = (**messages).id();
    let mut action = signed(
        update(
            id,
            parent,
            MainStorage::storage_read(Key::Entry(id)).expect("stored"),
        ),
        account_of_key(&alice),
        rules_of(id),
        &alice,
        later(),
    );
    if let Action::Update { metadata, .. } = &mut action {
        if let StorageType::User { rules, .. } = &mut metadata.storage_type {
            *rules = EntryRules::OWNED;
        }
    }
    assert!(apply(action, account_of_key(&alice)).is_err());
}

#[test]
#[serial]
fn an_entry_with_other_rules_is_never_read() {
    // A patched author writes an ordinary, editable entry into a write-once
    // collection. Correctly signed, so every node stores it; nobody reads it.
    let (messages, _alice, _id) = alices_message();
    let parent = (**messages).id();
    let mallory = key(0xEE);
    let id = crate::collections::compute_id(parent, b"sneaky");
    let add = signed(
        |metadata| Action::Add {
            id,
            data: entry_bytes(id, &"sneaky".to_owned(), &text("editable later")),
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        account_of_key(&mallory),
        EntryRules::OWNED,
        &mallory,
        later(),
    );
    apply(add, account_of_key(&mallory)).expect("a well-signed add applies");
    assert!(messages.get(&"sneaky".to_owned()).expect("get").is_none());
    assert_eq!(messages.len().expect("len"), 1);
}

#[test]
#[serial]
fn a_write_once_entry_seals_the_collections_inside_it() {
    env::reset_for_testing();
    let _ = act_as(&key(0xA1));
    let mut logs = Root::new(
        WriteOnce::<UnorderedMap<String, UnorderedMap<String, LwwRegister<String>>>>::new,
    );
    let mut filled = UnorderedMap::new();
    let _ = filled.insert("a".to_owned(), text("x")).expect("insert");
    assert!(refused(logs.insert("full".to_owned(), filled)));
    logs.insert("empty".to_owned(), UnorderedMap::new())
        .expect("insert");
    let mut inner = logs.get(&"empty".to_owned()).expect("get").expect("stored");
    assert!(refused(inner.insert("b".to_owned(), text("y"))));
}

#[test]
#[serial]
fn an_authored_map_does_not_read_write_once_entries() {
    // Exact rules: an editable collection ignores entries claiming stricter
    // ones, as a stricter collection ignores looser ones.
    env::reset_for_testing();
    let alice = key(0xA1);
    let _ = act_as(&alice);
    let notes = Root::new(Authored::<UnorderedMap<String, LwwRegister<String>>>::new);
    let parent = (**notes).id();
    let id = crate::collections::compute_id(parent, b"n");
    let add = signed(
        |metadata| Action::Add {
            id,
            data: entry_bytes(id, &"n".to_owned(), &text("strict")),
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        account_of_key(&alice),
        EntryRules {
            immutable: true,
            moderators: None,
        },
        &alice,
        later(),
    );
    apply(add, account_of_key(&alice)).expect("applies");
    assert!(notes.get(&"n".to_owned()).expect("get").is_none());
}

// ---------------------------------------------------------------------------
// Moderated
// ---------------------------------------------------------------------------

/// A board whose sole moderator is Mod, with a post by Bob.
fn board() -> (Root<Board>, SigningKey, SigningKey, Id) {
    env::reset_for_testing();
    let moderator = key(0x40);
    let bob = key(0xB0);
    let _ = act_as(&moderator);
    let mut board = Root::new(Board::new);
    let _ = act_as(&bob);
    board
        .insert("p".to_owned(), text("buy now"))
        .expect("bob posts");
    let id = board.entry_id(&"p".to_owned());
    (board, moderator, bob, id)
}

#[test]
#[serial]
fn a_moderated_entry_names_the_moderators() {
    let (board, moderator, bob, id) = board();
    let rules = rules_of(id);
    assert!(!rules.immutable);
    assert!(rules.moderators.is_some());
    assert_eq!(
        board.moderators(),
        [account_of_key(&moderator)].into_iter().collect()
    );
    assert!(board.is_moderator(&account_of_key(&moderator)));
    assert!(!board.is_moderator(&account_of_key(&bob)));
}

#[test]
#[serial]
fn the_owner_edits_and_a_stranger_cannot() {
    let (mut board, _moderator, _bob, _id) = board();
    board
        .update(&"p".to_owned(), text("edited by bob"))
        .expect("bob edits his post");
    let _ = act_as(&key(0xEE));
    assert!(refused(board.update(&"p".to_owned(), text("mallory"))));
    assert!(refused(board.remove(&"p".to_owned())));
}

#[test]
#[serial]
fn a_moderator_removes_someone_else_s_entry() {
    let (mut board, moderator, _bob, id) = board();
    let _ = act_as(&moderator);
    assert!(board
        .remove(&"p".to_owned())
        .expect("moderator removes")
        .is_some());
    assert!(board.get(&"p".to_owned()).expect("get").is_none());
    assert!(is_gone(id));
}

#[test]
#[serial]
fn every_node_accepts_a_moderator_s_signed_delete() {
    let (board, moderator, bob, id) = board();
    let at = later();
    let removal = signed(
        delete(id, at),
        account_of_key(&bob),
        rules_of(id),
        &moderator,
        at,
    );
    apply(removal, account_of_key(&moderator)).expect("a moderator's delete applies");
    assert!(board.get(&"p".to_owned()).expect("get").is_none());
}

#[test]
#[serial]
fn no_node_accepts_a_stranger_s_signed_delete() {
    let (board, _moderator, bob, id) = board();
    let mallory = key(0xEE);
    let at = later();
    let removal = signed(
        delete(id, at),
        account_of_key(&bob),
        rules_of(id),
        &mallory,
        at,
    );
    assert!(apply(removal, account_of_key(&mallory)).is_err());
    assert!(board.get(&"p".to_owned()).expect("get").is_some());
}

#[test]
#[serial]
fn a_revoked_moderator_can_no_longer_remove() {
    let (mut board, moderator, bob, id) = board();
    let other = key(0x41);
    let _ = act_as(&moderator);
    board
        .set_moderators([account_of_key(&other)].into_iter().collect())
        .expect("a moderator hands over");
    assert!(!board.is_moderator(&account_of_key(&moderator)));
    assert!(refused(board.remove(&"p".to_owned())), "locally");

    let at = later();
    let removal = signed(
        delete(id, at),
        account_of_key(&bob),
        rules_of(id),
        &moderator,
        at,
    );
    assert!(
        apply(removal, account_of_key(&moderator)).is_err(),
        "on every node"
    );
    assert!(board.get(&"p".to_owned()).expect("get").is_some());
}

#[test]
#[serial]
fn only_a_moderator_may_change_the_moderators() {
    let (mut board, _moderator, bob, _id) = board();
    let _ = act_as(&bob);
    assert!(board
        .set_moderators([account_of_key(&bob)].into_iter().collect())
        .is_err());
}

#[test]
#[serial]
fn the_owner_may_delete_an_editable_moderated_entry() {
    let (mut board, _moderator, bob, id) = board();
    let _ = act_as(&bob);
    assert!(board
        .remove(&"p".to_owned())
        .expect("bob removes")
        .is_some());
    assert!(is_gone(id));
}

#[test]
#[serial]
fn an_entry_escaping_moderation_is_never_read() {
    // A patched author writes into the board with no moderators named, so no
    // moderator could remove it. Every node stores it; nobody reads it.
    let (board, _moderator, _bob, _id) = board();
    let parent = (**board).id();
    let mallory = key(0xEE);
    let id = crate::collections::compute_id(parent, b"spam");
    let add = signed(
        |metadata| Action::Add {
            id,
            data: entry_bytes(id, &"spam".to_owned(), &text("unremovable")),
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        },
        account_of_key(&mallory),
        EntryRules::OWNED,
        &mallory,
        later(),
    );
    apply(add, account_of_key(&mallory)).expect("a well-signed add applies");
    assert!(board.get(&"spam".to_owned()).expect("get").is_none());
    let keys: Vec<_> = board.entries().expect("entries").map(|(k, _)| k).collect();
    assert_eq!(keys, ["p"]);
}

// ---------------------------------------------------------------------------
// Moderated and written once
// ---------------------------------------------------------------------------

fn chat() -> (Root<Chat>, SigningKey, SigningKey, Id) {
    env::reset_for_testing();
    let moderator = key(0x40);
    let bob = key(0xB0);
    let _ = act_as(&moderator);
    let mut chat = Root::new(Chat::new);
    let _ = act_as(&bob);
    chat.insert("c".to_owned(), text("spam"))
        .expect("bob writes");
    let id = chat.entry_id(&"c".to_owned());
    (chat, moderator, bob, id)
}

#[test]
#[serial]
fn nobody_edits_a_chat_message_and_its_author_cannot_delete_it() {
    let (mut chat, _moderator, bob, id) = chat();
    assert!(rules_of(id).immutable);
    assert!(refused(chat.remove(&"c".to_owned())), "author, locally");

    let at = later();
    let removal = signed(delete(id, at), account_of_key(&bob), rules_of(id), &bob, at);
    assert!(
        apply(removal, account_of_key(&bob)).is_err(),
        "author, on every node"
    );
    let parent = (**chat).id();
    let rewrite = signed(
        update(
            id,
            parent,
            entry_bytes(id, &"c".to_owned(), &text("edited")),
        ),
        account_of_key(&bob),
        rules_of(id),
        &bob,
        later(),
    );
    assert!(
        apply(rewrite, account_of_key(&bob)).is_err(),
        "edit, on every node"
    );
    assert!(chat.get(&"c".to_owned()).expect("get").is_some());
}

#[test]
#[serial]
fn a_moderator_still_removes_a_chat_message() {
    let (mut chat, moderator, bob, id) = chat();
    let at = later();
    let removal = signed(
        delete(id, at),
        account_of_key(&bob),
        rules_of(id),
        &moderator,
        at,
    );
    apply(removal, account_of_key(&moderator)).expect("a moderator's delete applies");
    assert!(chat.get(&"c".to_owned()).expect("get").is_none());

    let _ = act_as(&key(0xB0));
    chat.insert("d".to_owned(), text("again"))
        .expect("bob writes again");
    let _ = act_as(&moderator);
    assert!(chat.remove(&"d".to_owned()).expect("locally").is_some());
}

#[test]
#[serial]
fn a_plain_authored_entry_is_unchanged() {
    env::reset_for_testing();
    let alice = key(0xA1);
    let _ = act_as(&alice);
    let mut notes = Root::new(Authored::<UnorderedMap<String, LwwRegister<String>>>::new);
    notes.insert("n".to_owned(), text("v1")).expect("insert");
    let id = notes.entry_id(&"n".to_owned());
    assert_eq!(rules_of(id), EntryRules::OWNED);
    notes
        .update(&"n".to_owned(), text("v2"))
        .expect("owner edits");
    assert!(notes
        .remove(&"n".to_owned())
        .expect("owner removes")
        .is_some());
}
