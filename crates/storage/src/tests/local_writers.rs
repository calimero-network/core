//! A write to a writer-set-guarded entity is refused on the local write path
//! when the caller's account lacks the operation in the cell's current writers.
//!
//! release-v2 runs an app once, on the leader, and commits what it wrote; no
//! peer re-checks the write at merge. So the collections refuse it before it is
//! stored, and the app's method gets the error: `SharedStorage`,
//! `PermissionedStorage` (and the map a cell holds, written through `get_mut`),
//! `Ownable`, and a write-once cell. The writers are the ones the merge path
//! resolves, which include a rotation the same run asked for.

use std::collections::{BTreeMap, BTreeSet};

use calimero_account::AccountId;
use serial_test::serial;

use crate::collections::{
    LwwRegister, Ownable, PermissionedStorage, ProtocolAuthorizer, Root, SharedStorage, StoreError,
    UnorderedMap,
};
use crate::entities::OpMask;
use crate::env;
use crate::index::Index;
use crate::interface::StorageError;
use crate::store::MainStorage;

type Settings = UnorderedMap<String, LwwRegister<String>>;

const ALICE: [u8; 32] = [0xA1; 32];
const BOB: [u8; 32] = [0xB0; 32];

fn account(bytes: [u8; 32]) -> AccountId {
    AccountId::from(bytes)
}

fn act_as(bytes: [u8; 32]) {
    env::set_account_id(bytes);
}

fn only(bytes: [u8; 32]) -> BTreeSet<AccountId> {
    BTreeSet::from([account(bytes)])
}

fn is_refusal<T: core::fmt::Debug>(result: &Result<T, StoreError>) -> bool {
    matches!(
        result,
        Err(StoreError::StorageError(StorageError::ActionNotAllowed(_)))
    )
}

fn stored(id: crate::address::Id) -> bool {
    <Index<MainStorage>>::get_metadata(id)
        .expect("metadata")
        .is_some()
}

/// Alice's settings cell holding `seed = alice`.
fn alices_settings() -> Root<SharedStorage<Settings>> {
    env::reset_for_testing();
    act_as(ALICE);
    let mut cell = Root::new(|| SharedStorage::<Settings>::new(only(ALICE), false));
    let _ = cell
        .get_mut()
        .expect("writer")
        .insert("seed".to_owned(), LwwRegister::new("alice".to_owned()))
        .expect("alice writes");
    cell
}

#[test]
#[serial]
fn a_non_writers_map_insert_is_refused_and_stores_nothing() {
    let mut cell = alices_settings();
    act_as(BOB);
    let refused = cell
        .get_mut()
        .expect("the cell hands out its map")
        .insert("evil".to_owned(), LwwRegister::new("forged".to_owned()));
    assert!(is_refusal(&refused), "{refused:?}");

    let map = cell.get().expect("get");
    assert!(map.get("evil").expect("get").is_none());
    assert!(!stored(map.entry_id("evil")), "nothing is stored for it");
}

#[test]
#[serial]
fn a_non_writers_map_edit_in_place_is_refused() {
    let mut cell = alices_settings();
    act_as(BOB);
    let refused = cell
        .get_mut()
        .expect("the cell hands out its map")
        .get_mut("seed")
        .map(|edit| edit.is_some());
    assert!(is_refusal(&refused), "{refused:?}");
    let map = cell.get().expect("get");
    assert_eq!(
        map.get("seed").expect("get").map(|v| v.get().clone()),
        Some("alice".to_owned())
    );
}

#[test]
#[serial]
fn a_non_writers_map_remove_is_refused() {
    let mut cell = alices_settings();
    act_as(BOB);
    let refused = cell.get_mut().expect("map").remove("seed");
    assert!(is_refusal(&refused), "{refused:?}");
    let map = cell.get().expect("get");
    assert!(map.get("seed").expect("get").is_some(), "the entry stays");
}

#[test]
#[serial]
fn a_writers_map_writes_still_land() {
    let mut cell = alices_settings();
    let map = cell.get_mut().expect("map");
    let _ = map
        .insert("theme".to_owned(), LwwRegister::new("dark".to_owned()))
        .expect("insert");
    let _ = map.remove("seed").expect("remove");
    let map = cell.get().expect("get");
    assert!(map.get("theme").expect("get").is_some());
    assert!(map.get("seed").expect("get").is_none());
}

#[test]
#[serial]
fn a_refused_write_leaves_the_runs_other_writes_alone() {
    let mut cell = alices_settings();
    let mut open = Root::new(UnorderedMap::<String, String>::new);
    act_as(BOB);
    // The app catches the refusal and carries on: its other writes stand.
    let refused = cell
        .get_mut()
        .expect("map")
        .insert("evil".to_owned(), LwwRegister::new("forged".to_owned()));
    assert!(is_refusal(&refused));
    let _ = open
        .insert("note".to_owned(), "bob was here".to_owned())
        .expect("an unguarded write");
    assert_eq!(
        open.get("note").expect("get").map(|v| v.clone()),
        Some("bob was here".to_owned())
    );
    assert!(cell.get().expect("get").get("evil").expect("get").is_none());
}

#[test]
#[serial]
fn a_writer_without_delete_cannot_remove() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut cell =
        Root::new(|| PermissionedStorage::<Settings, ProtocolAuthorizer>::new(only(ALICE), false));
    let _ = cell
        .get_mut()
        .expect("map")
        .insert("k".to_owned(), LwwRegister::new("v".to_owned()))
        .expect("insert");
    // Alice keeps WRITE and gives up DELETE (and ADMIN).
    cell.set_capabilities(BTreeMap::from([
        (account(ALICE), OpMask::WRITE),
        (account(BOB), OpMask::FULL),
    ]))
    .expect("alice is admin");

    let map = cell.get_mut().expect("map");
    let _ = map
        .insert("k2".to_owned(), LwwRegister::new("v2".to_owned()))
        .expect("WRITE still writes");
    let refused = map.remove("k");
    assert!(is_refusal(&refused), "no DELETE, no removal: {refused:?}");
}

#[test]
#[serial]
fn rotate_then_write_in_one_run_is_checked_against_the_new_set() {
    let mut cell = alices_settings();
    // Alice hands the cell to Bob, then writes in the same run: refused, because
    // the run reads its own rotation back.
    cell.rotate_writers(only(BOB)).expect("alice is admin");
    let refused = cell
        .get_mut()
        .expect("map")
        .insert("after".to_owned(), LwwRegister::new("rotation".to_owned()));
    assert!(is_refusal(&refused), "{refused:?}");
    let refused = cell.insert(Settings::new());
    assert!(is_refusal(&refused), "{refused:?}");

    // Bob, whom the rotation added, writes.
    act_as(BOB);
    let _ = cell
        .get_mut()
        .expect("map")
        .insert("bob".to_owned(), LwwRegister::new("writes".to_owned()))
        .expect("the new writer writes");
}

#[test]
#[serial]
fn rotate_in_a_writer_then_write_in_one_run() {
    env::reset_for_testing();
    act_as(ALICE);
    // Alice holds ADMIN only; the cell is created with her as a full writer and
    // then scoped down, so her own write depends on the set in effect.
    let mut cell =
        Root::new(|| PermissionedStorage::<Settings, ProtocolAuthorizer>::new(only(ALICE), false));
    cell.set_capabilities(BTreeMap::from([(account(ALICE), OpMask::ADMIN)]))
        .expect("admin");
    let refused = cell
        .get_mut()
        .expect("map")
        .insert("k".to_owned(), LwwRegister::new("v".to_owned()));
    assert!(
        is_refusal(&refused),
        "ADMIN alone does not write: {refused:?}"
    );

    // She grants herself WRITE and writes in the same run.
    cell.grant_capability(account(ALICE), OpMask::ADMIN.union(OpMask::WRITE))
        .expect("admin");
    let _ = cell
        .get_mut()
        .expect("map")
        .insert("k".to_owned(), LwwRegister::new("v".to_owned()))
        .expect("the new set grants WRITE");
}

#[test]
#[serial]
fn ownable_refuses_a_non_owners_insert() {
    env::reset_for_testing();
    act_as(ALICE);
    let mut config = Root::new(|| Ownable::<LwwRegister<String>>::new_owned_by(account(ALICE)));
    let _ = config
        .insert(LwwRegister::new("alice".to_owned()))
        .expect("owner");
    act_as(BOB);
    let refused = config.insert(LwwRegister::new("bob".to_owned()));
    assert!(is_refusal(&refused), "{refused:?}");
    assert_eq!(config.get().expect("get").get(), "alice");
}

#[test]
#[serial]
fn a_cell_created_for_others_still_constructs() {
    // Genesis is the constructor's, whoever it names; it guards nothing yet.
    env::reset_for_testing();
    act_as(ALICE);
    let mut config = Root::new(|| Ownable::<LwwRegister<String>>::new_owned_by(account(BOB)));
    let refused = config.insert(LwwRegister::new("alice".to_owned()));
    assert!(is_refusal(&refused), "alice is not the owner: {refused:?}");
    act_as(BOB);
    let _ = config
        .insert(LwwRegister::new("bob".to_owned()))
        .expect("the owner writes");
}

/// `Element::set_shared_member_domain` is public, so app code can stamp one of
/// its collections a member of an anchor that does not exist. Through the
/// collections nothing lands: the local check resolves that anchor's writers to
/// the empty set, and the made-up anchor is not a cell's creation (neither the
/// anchor's own id nor its value id).
#[test]
#[serial]
fn a_member_of_a_made_up_anchor_is_refused_through_the_collections() {
    use crate::entities::Data;

    env::reset_for_testing();
    act_as(ALICE);
    let fake = crate::address::Id::new([0x5A; 32]);
    let mut map = Root::new(Settings::new);
    (*map).element_mut().set_shared_member_domain(fake);
    let refused = map.insert("k".to_owned(), LwwRegister::new("v".to_owned()));
    assert!(is_refusal(&refused), "{refused:?}");
    assert!(!stored(map.entry_id("k")), "nothing is stored for it");

    // Saving the map itself as such a member is refused the same way: it is
    // already stored, so this is no creation.
    let saved = crate::interface::Interface::<MainStorage>::save(&mut *map);
    assert!(
        matches!(saved, Err(StorageError::ActionNotAllowed(_))),
        "{saved:?}"
    );
}

/// What the creation exemption does let app code do, through the raw
/// `Interface` with an element it builds itself: store a member at the very id
/// it names as its anchor (or at that anchor's value id). That is a cell's
/// creation as the local check sees it. The entity's writers then resolve to
/// the empty set (its anchor is no `Shared` entity), so nobody can change or
/// remove it afterwards. This is the app writing its own state, which #4721's
/// ruling accepts: it froze an entity of its own and reached nobody else's.
#[test]
#[serial]
fn a_self_anchored_member_from_the_raw_interface_lands_and_is_frozen() {
    use crate::entities::{Data, Element, StorageType};
    use crate::interface::Interface;
    use crate::tests::common::Paragraph;

    env::reset_for_testing();
    act_as(ALICE);
    let _root = Root::new(Settings::new);
    let id = crate::address::Id::new([0x5B; 32]);
    let mut element = Element::new(Some(id));
    element.set_shared_member_domain(id);
    let mut member = Paragraph::new_from_element("mine", element);
    let added = Interface::<MainStorage>::add_child_to(crate::address::Id::root(), &mut member);
    assert!(matches!(added, Ok(true)), "{added:?}");
    assert!(matches!(
        <Index<MainStorage>>::get_metadata(id).unwrap().unwrap().storage_type,
        StorageType::SharedMember { anchor, .. } if anchor == id
    ));

    // Its writers are nobody, so no account changes or removes it, its
    // creator included.
    for who in [ALICE, BOB] {
        act_as(who);
        member.text = format!("changed by {who:?}");
        member.element_mut().update();
        let saved = Interface::<MainStorage>::save(&mut member);
        assert!(
            matches!(saved, Err(StorageError::ActionNotAllowed(_))),
            "{saved:?}"
        );
        let removed = Interface::<MainStorage>::remove_child_from(crate::address::Id::root(), id);
        assert!(
            matches!(removed, Err(StorageError::ActionNotAllowed(_))),
            "{removed:?}"
        );
    }
    assert!(stored(id));
}
