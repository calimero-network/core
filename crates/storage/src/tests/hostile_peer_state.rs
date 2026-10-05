//! A peer's root, app-state entry and register stamp meet the rules of every
//! remote write: a stamp within the drift bound, and bytes the entry's type reads.

use core::num::NonZeroU64;
use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use borsh::{from_slice, to_vec};
use calimero_account::AccountId;
use serial_test::serial;

use crate::action::Action;
use crate::address::Id;
use crate::collections::crdt_meta::{CrdtType, CustomTypeId};
use crate::collections::{LwwRegister, Root, ROOT_ENTRY_ID};
use crate::constants::DRIFT_TOLERANCE_NANOS;
use crate::delta::StorageDelta;
use crate::entities::Metadata;
use crate::env;
use crate::index::Index;
use crate::interface::{ApplyContext, Interface, StorageError};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::merge::registry::clear_merge_registry;
use crate::merge::{merge_root_state_typed, register_crdt_merge, MergeRootStateResponse};
use crate::store::{Key, MainStorage, StorageAdaptor};
use crate::tests::common::account_of_key;
use crate::tests::owned_rules::{apply, key, later, text};

type Reg = LwwRegister<String>;

const MALLORY: u8 = 0xEE;

fn stored(id: Id) -> Metadata {
    <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .expect("stored")
}

fn mallory() -> AccountId {
    account_of_key(&key(MALLORY))
}

/// A peer's rewrite of the stored entry at `id` with `data`, stamped `at`.
fn peer_update(id: Id, data: Vec<u8>, at: u64) -> Action {
    let mut metadata = stored(id);
    metadata.updated_at = at.into();
    Action::Update {
        id,
        data,
        ancestors: <Index<MainStorage>>::get_ancestors_of(id).expect("ancestors"),
        metadata,
    }
}

// ---------------------------------------------------------------------------
// The root and the app state
// ---------------------------------------------------------------------------

/// An app whose state is one register, registered and initialised as `init` does.
fn genesis() {
    env::reset_for_testing();
    clear_merge_registry();
    register_crdt_merge::<Reg>();
    let _genesis = Root::new(|| text("genesis"));
}

fn root_bytes() -> Vec<u8> {
    MainStorage::storage_read(Key::Entry(Id::root())).expect("root bytes")
}

/// Mallory's delta whose root action carries `data`, stamped `updated_at`.
fn sync_root(data: Vec<u8>, updated_at: u64) -> Result<(), StorageError> {
    let mut metadata = stored(Id::root());
    metadata.updated_at = updated_at.into();
    let delta = StorageDelta::CausalActions {
        actions: vec![Action::Update {
            id: Id::root(),
            data,
            ancestors: vec![],
            metadata,
        }],
        delta_id: [0xC1; 32],
        delta_hlc: env::hlc_timestamp(),
        effective_writers: BTreeMap::new(),
        signer_account: Some(mallory()),
        on_behalf_accounts: BTreeMap::new(),
    };
    Root::<Reg>::sync(&to_vec(&delta).expect("delta"), &ApplyContext::empty())
}

/// Mallory's delta deleting the stored entry at `id`.
fn sync_delete(id: Id) {
    let delta = StorageDelta::CausalActions {
        actions: vec![Action::DeleteRef {
            id,
            deleted_at: env::time_now(),
            metadata: stored(id),
        }],
        delta_id: [0xC2; 32],
        delta_hlc: env::hlc_timestamp(),
        effective_writers: BTreeMap::new(),
        signer_account: Some(mallory()),
        on_behalf_accounts: BTreeMap::new(),
    };
    let _ = Root::<Reg>::sync(&to_vec(&delta).expect("delta"), &ApplyContext::empty());
}

/// A local method call writing the app state; true when it committed.
fn local_write_commits(value: &str) -> bool {
    let _ = env::take_last_artifact();
    let mut app = Root::<Reg>::fetch().expect("root");
    app.set(value.to_owned());
    app.commit();
    env::take_last_artifact().is_some()
}

/// The stored bytes of the app-state entry holding `value`.
fn app_state(value: &str) -> Vec<u8> {
    let mut data = to_vec(&text(value)).expect("value");
    data.extend(to_vec(&ROOT_ENTRY_ID).expect("id"));
    data
}

fn app_value() -> String {
    Reg::get(&Root::<Reg>::fetch().expect("root")).clone()
}

#[test]
#[serial]
fn a_root_action_stamped_far_ahead_does_not_freeze_local_writes() {
    genesis();
    sync_root(root_bytes(), env::time_now()).expect("control: a current root action applies");
    assert!(
        local_write_commits("after"),
        "control: a later local write commits"
    );

    let _ = sync_root(root_bytes(), u64::MAX);

    assert!(
        local_write_commits("after again"),
        "a far-future root stamp must not drop every later local write"
    );
}

#[test]
#[serial]
fn a_root_action_stamped_inside_the_bound_does_not_hold_back_local_writes() {
    genesis();
    let ahead = env::time_now() + DRIFT_TOLERANCE_NANOS - 1_000_000_000;

    sync_root(root_bytes(), ahead).expect("a restated shell inside the bound is accepted");

    assert!(
        local_write_commits("right after"),
        "a local write right after a restated shell must still commit"
    );
}

#[test]
#[serial]
fn a_root_shell_stamped_far_ahead_is_refused_when_no_root_is_stored() {
    genesis();
    let root_shell = root_bytes();
    env::reset_for_testing();
    let shell = |at: u64| Action::Add {
        id: Id::root(),
        data: root_shell.clone(),
        ancestors: vec![],
        metadata: Metadata::new(0, at),
    };

    let refused =
        <Interface<MainStorage>>::apply_remote_action(shell(u64::MAX), &ApplyContext::empty());

    assert!(
        matches!(refused, Err(StorageError::InvalidTimestamp(..))),
        "a far-future shell must be refused for its stamp, got {refused:?}"
    );
    assert!(<Index<MainStorage>>::get_metadata(Id::root())
        .expect("index")
        .is_none());
    <Interface<MainStorage>>::apply_remote_action(shell(env::time_now()), &ApplyContext::empty())
        .expect("control: a current shell applies on an empty store");
}

#[test]
#[serial]
fn a_root_action_with_undecodable_bytes_does_not_brick_the_root() {
    genesis();
    sync_root(root_bytes(), env::time_now()).expect("control: a current root action applies");
    assert!(Root::<Reg>::fetch().is_some(), "control: the root reads");

    let _ = sync_root(vec![0xFF; 3], env::time_now());

    let reads = catch_unwind(AssertUnwindSafe(|| Root::<Reg>::fetch().is_some()));
    assert!(
        matches!(reads, Ok(true)),
        "undecodable root bytes must not replace a readable root"
    );
}

#[test]
#[serial]
fn a_root_action_with_another_shell_does_not_replace_the_stored_one() {
    genesis();
    let shell = root_bytes();

    let _ = sync_root(vec![], env::time_now());

    assert_eq!(
        root_bytes(),
        shell,
        "an empty shell must not replace a `Root<T>` shell"
    );
    assert!(Root::<Reg>::fetch().is_some(), "the root still reads");
}

#[test]
#[serial]
fn a_root_shell_naming_a_crdt_type_is_refused_when_no_root_is_stored() {
    env::reset_for_testing();
    let shell = |crdt_type: Option<CrdtType>| Action::Add {
        id: Id::root(),
        data: [
            Id::root().as_bytes().as_slice(),
            &to_vec(&crdt_type).expect("type"),
        ]
        .concat(),
        ancestors: vec![],
        metadata: Metadata::new(0, env::time_now()),
    };

    let refused = <Interface<MainStorage>>::apply_remote_action(
        shell(Some(CrdtType::GCounter)),
        &ApplyContext::empty(),
    );

    assert!(
        matches!(refused, Err(StorageError::InvalidData(..))),
        "a root collection naming a CRDT type is not a shell, got {refused:?}"
    );
    <Interface<MainStorage>>::apply_remote_action(shell(None), &ApplyContext::empty())
        .expect("control: an untyped root collection applies on an empty store");
}

#[test]
#[serial]
fn undecodable_app_state_does_not_replace_a_valid_one() {
    genesis();
    let at = later();
    apply(peer_update(ROOT_ENTRY_ID, app_state("peer"), at), mallory())
        .expect("control: a peer's app state applies");
    assert_eq!(app_value(), "peer", "control: the peer's app state reads");

    let _ = apply(peer_update(ROOT_ENTRY_ID, vec![0xFF; 3], at + 1), mallory());

    let value = catch_unwind(app_value);
    assert_eq!(
        value.ok().as_deref(),
        Some("peer"),
        "undecodable app state must not replace a valid one"
    );
}

#[test]
#[serial]
fn undecodable_app_state_restating_the_stored_bytes_is_still_refused() {
    genesis();
    let entry_bytes = MainStorage::storage_read(Key::Entry(ROOT_ENTRY_ID)).expect("entry");
    let mut garbage = vec![0xFF; 3];
    garbage.extend(to_vec(&ROOT_ENTRY_ID).expect("id"));
    MainStorage::storage_write(Key::Entry(ROOT_ENTRY_ID), &garbage);

    let refused = apply(peer_update(ROOT_ENTRY_ID, garbage, later()), mallory());

    assert!(
        matches!(refused, Err(StorageError::InvalidData(_))),
        "bytes the app type does not read are refused even when already stored, got {refused:?}"
    );
    MainStorage::storage_write(Key::Entry(ROOT_ENTRY_ID), &entry_bytes);
}

#[test]
#[serial]
fn a_repaired_app_state_entry_stamped_far_ahead_is_refused() {
    genesis();
    <Interface<MainStorage>>::root_entry_merge_request(app_state("peer"), env::time_now())
        .expect("control: a current stamp passes");

    let refused = <Interface<MainStorage>>::root_entry_merge_request(app_state("peer"), u64::MAX);

    assert!(
        matches!(refused, Err(StorageError::InvalidTimestamp(..))),
        "a far-future stamp must be refused before any merge, got {refused:?}"
    );
}

#[test]
#[serial]
fn a_repaired_app_state_entry_with_undecodable_bytes_is_refused() {
    genesis();
    let request = |entry| {
        <Interface<MainStorage>>::root_entry_merge_request(entry, env::time_now()).expect("request")
    };
    assert!(
        matches!(
            merge_root_state_typed::<Reg>(&request(app_state("peer"))),
            MergeRootStateResponse::Ok(_)
        ),
        "control: a readable entry merges"
    );

    let mut garbage = vec![0xFF; 3];
    garbage.extend(to_vec(&ROOT_ENTRY_ID).expect("id"));
    let response = merge_root_state_typed::<Reg>(&request(garbage));

    assert!(
        matches!(response, MergeRootStateResponse::Refused(_)),
        "bytes the app type does not read are refused on repair as on a delta, got {response:?}"
    );
}

#[test]
#[serial]
fn a_repaired_app_state_entry_does_not_overwrite_a_write_made_during_its_merge() {
    genesis();
    let request = <Interface<MainStorage>>::root_entry_merge_request(app_state("peer"), later())
        .expect("request");
    let merged = app_state("merged");
    assert!(
        local_write_commits("local"),
        "a write stamped below the peer's lands mid-merge"
    );

    let written = <Interface<MainStorage>>::write_root_entry_merge(&request, Some(&merged), 0);

    assert!(
        matches!(written, Ok(None)),
        "nothing is written, got {written:?}"
    );
    assert_eq!(
        app_value(),
        "local",
        "a merge of a stored entry that has since moved must not replace it"
    );
}

#[test]
#[serial]
fn a_repaired_custom_entry_stamped_far_ahead_is_refused() {
    genesis();
    let request = |at| {
        <Interface<MainStorage>>::custom_entry_merge_request(
            ROOT_ENTRY_ID,
            CustomTypeId::of("app::Custom"),
            b"peer".to_vec(),
            at,
        )
    };
    assert!(
        matches!(request(env::time_now()), Ok(Some(_))),
        "control: a current stamp passes"
    );

    let refused = request(u64::MAX);

    assert!(
        matches!(refused, Err(StorageError::InvalidTimestamp(..))),
        "a far-future stamp must be refused before any merge, got {refused:?}"
    );
}

#[test]
#[serial]
fn a_repaired_custom_entry_does_not_overwrite_a_write_made_during_its_merge() {
    genesis();
    let incoming_ts = later();
    let (request, stored_metadata) = <Interface<MainStorage>>::custom_entry_merge_request(
        ROOT_ENTRY_ID,
        CustomTypeId::of("app::Custom"),
        app_state("peer"),
        incoming_ts,
    )
    .expect("request")
    .expect("an entry is stored");
    let merged = app_state("merged");
    assert!(
        local_write_commits("local"),
        "a write stamped below the peer's lands mid-merge"
    );

    let written = <Interface<MainStorage>>::write_custom_entry_merge(
        ROOT_ENTRY_ID,
        &request,
        &stored_metadata,
        &merged,
        incoming_ts,
    );

    assert!(
        matches!(written, Ok(None)),
        "nothing is written, got {written:?}"
    );
    assert_eq!(
        app_value(),
        "local",
        "a merge of a stored entry that has since moved must not replace it"
    );
}

#[test]
#[serial]
fn a_repaired_custom_entry_is_written_when_the_entry_did_not_move() {
    genesis();
    let (request, stored_metadata) = <Interface<MainStorage>>::custom_entry_merge_request(
        ROOT_ENTRY_ID,
        CustomTypeId::of("app::Custom"),
        app_state("peer"),
        later(),
    )
    .expect("request")
    .expect("an entry is stored");

    let written = <Interface<MainStorage>>::write_custom_entry_merge(
        ROOT_ENTRY_ID,
        &request,
        &stored_metadata,
        &app_state("merged"),
        later(),
    );

    assert!(matches!(written, Ok(Some(_))), "got {written:?}");
    assert_eq!(app_value(), "merged");
}

/// The stamp a merge write-back takes is the peer's, so the write bounds it as the
/// request does: a merged entry dated past it would outdate every later write.
#[test]
#[serial]
fn a_custom_entry_written_back_with_a_far_future_stamp_is_refused() {
    genesis();
    let before = app_value();
    let (request, stored_metadata) = <Interface<MainStorage>>::custom_entry_merge_request(
        ROOT_ENTRY_ID,
        CustomTypeId::of("app::Custom"),
        app_state("peer"),
        later(),
    )
    .expect("request")
    .expect("an entry is stored");

    let written = <Interface<MainStorage>>::write_custom_entry_merge(
        ROOT_ENTRY_ID,
        &request,
        &stored_metadata,
        &app_state("merged"),
        u64::MAX,
    );

    assert!(
        matches!(written, Err(StorageError::InvalidTimestamp(..))),
        "got {written:?}"
    );
    assert_eq!(app_value(), before);
}

/// As for a custom entry: the app-state write-back bounds the peer stamp it carries.
#[test]
#[serial]
fn an_app_state_entry_written_back_with_a_far_future_stamp_is_refused() {
    genesis();
    let before = app_value();
    let mut request =
        <Interface<MainStorage>>::root_entry_merge_request(app_state("peer"), later())
            .expect("request");
    request.incoming_ts = u64::MAX;

    let written =
        <Interface<MainStorage>>::write_root_entry_merge(&request, Some(&app_state("merged")), 0);

    assert!(
        matches!(written, Err(StorageError::InvalidTimestamp(..))),
        "got {written:?}"
    );
    assert_eq!(app_value(), before);
}

// ---------------------------------------------------------------------------
// A register's own stamp
// ---------------------------------------------------------------------------

/// A register as a peer sends it, stamped `ntp` whatever the clock says.
fn register_stamped(value: &str, ntp: u64) -> Reg {
    let stamp = HybridTimestamp::new(Timestamp::new(NTP64(ntp), ID::from(NonZeroU64::MIN)));
    from_slice(&to_vec(&(value.to_owned(), stamp)).expect("register"))
        .expect("a register is its value and stamp")
}

#[test]
#[serial]
fn a_register_stamped_far_ahead_does_not_win_a_merge() {
    env::reset_for_testing();
    let stored = text("alice");
    let mut control = stored.clone();
    control.merge(&text("bob"));
    assert_eq!(control.get().as_str(), "bob", "control: a newer write wins");

    let mut merged = stored;
    merged.merge(&register_stamped("mallory", u64::MAX));

    assert_eq!(
        merged.get().as_str(),
        "alice",
        "a stamp beyond the drift bound must not win"
    );
}

#[test]
#[serial]
fn a_register_stamped_far_ahead_merges_the_same_on_either_side() {
    env::reset_for_testing();
    let honest = text("alice");
    let hostile = register_stamped("mallory", u64::MAX);

    let mut stored_honest = honest.clone();
    stored_honest.merge(&hostile);
    let mut stored_hostile = hostile.clone();
    stored_hostile.merge(&honest);

    assert_eq!(
        stored_honest.get(),
        stored_hostile.get(),
        "merge must not depend on which side holds the far-future stamp"
    );
    assert_eq!(stored_hostile.get().as_str(), "alice");
}

#[test]
#[serial]
fn a_peer_delete_of_the_app_state_entry_is_refused() {
    genesis();
    assert!(
        local_write_commits("local"),
        "control: a local write commits"
    );

    sync_delete(ROOT_ENTRY_ID);

    assert_eq!(app_value(), "local", "a peer must not delete the app state");
}

#[test]
#[serial]
fn a_peer_delete_of_the_root_is_refused() {
    genesis();
    let before = root_bytes();

    sync_delete(Id::root());

    assert_eq!(root_bytes(), before, "a peer must not delete the root");
    assert!(
        local_write_commits("after"),
        "local writes still commit after a refused root delete"
    );
}

/// A repair of the app-state entry by `value`, stamped `at`, for an app that merges nothing.
fn repair_without_merge(value: &str, at: u64) {
    let request =
        <Interface<MainStorage>>::root_entry_merge_request(app_state(value), at).expect("request");
    let written = <Interface<MainStorage>>::write_root_entry_merge(&request, None, 0);
    assert!(matches!(written, Ok(Some(_))), "{written:?}");
}

#[test]
#[serial]
fn a_repair_without_a_merge_keeps_a_newer_stored_entry() {
    genesis();
    assert!(
        local_write_commits("local"),
        "control: a local write commits"
    );
    let stored_at = *stored(ROOT_ENTRY_ID).updated_at;

    repair_without_merge("peer", stored_at - 1);

    assert_eq!(
        app_value(),
        "local",
        "an older peer entry must not replace a newer one"
    );
}

#[test]
#[serial]
fn a_repair_without_a_merge_breaks_an_equal_stamp_by_the_bytes() {
    genesis();
    assert!(local_write_commits("zzz"), "control: a local write commits");
    let stored_at = *stored(ROOT_ENTRY_ID).updated_at;

    repair_without_merge("aaa", stored_at);

    assert_eq!(
        app_value(),
        "zzz",
        "an equal stamp keeps the greater bytes on every node, not whichever arrived"
    );
}
