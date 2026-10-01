//! An `Update` to an entity its writer already held names no parent; the
//! receiver places it under the parent it stores, and drops one it cannot
//! place rather than failing the delta.

use borsh::to_vec;

use crate::action::Action;
use crate::address::Id;
use crate::child_trie::ChildTrie;
use crate::collections::Root;
use crate::delta::StorageDelta;
use crate::entities::{ChildInfo, Metadata};
use crate::index::Index;
use crate::interface::{ApplyContext, Interface};
use crate::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
use crate::store::{MockedStorage, StorageAdaptor};
use crate::tests::common::EmptyData;
use crate::{env, merge};

/// Receives every update with no parent named.
type Bare = MockedStorage<4810>;
/// Receives the same updates with the parent named, as a delta used to.
type Named = MockedStorage<4811>;

fn map_id() -> Id {
    Id::new([0x11; 32])
}

fn entry_id() -> Id {
    Id::new([0x22; 32])
}

fn parent(id: Id) -> ChildInfo {
    ChildInfo::new(id, [0; 32], Metadata::default())
}

fn upsert(add: bool, id: Id, data: &[u8], ancestors: Vec<ChildInfo>, at: u64) -> Action {
    let (data, metadata) = (data.to_vec(), Metadata::new(at, at));
    if add {
        Action::Add {
            id,
            data,
            ancestors,
            metadata,
        }
    } else {
        Action::Update {
            id,
            data,
            ancestors,
            metadata,
        }
    }
}

fn apply<S: StorageAdaptor>(action: Action) {
    Interface::<S>::apply_action(action, &ApplyContext::empty()).unwrap();
}

fn root_hash<S: StorageAdaptor>() -> [u8; 32] {
    Index::<S>::get_hashes_for(Id::root()).unwrap().unwrap().0
}

/// A store holding a map under the root and one entry in it.
fn seed<S: StorageAdaptor>() {
    Index::<S>::add_root(parent(Id::root())).unwrap();
    apply::<S>(upsert(
        true,
        map_id(),
        b"map",
        vec![parent(Id::root())],
        100,
    ));
    apply::<S>(upsert(true, entry_id(), b"v1", vec![parent(map_id())], 110));
}

fn setup() {
    env::reset_for_testing();
    merge::register_crdt_merge::<EmptyData>();
    seed::<Bare>();
    seed::<Named>();
    assert_eq!(root_hash::<Bare>(), root_hash::<Named>());
}

#[test]
fn an_update_naming_no_parent_lands_as_one_naming_it_does() {
    setup();

    apply::<Bare>(upsert(false, entry_id(), b"v2", vec![], 120));
    apply::<Named>(upsert(
        false,
        entry_id(),
        b"v2",
        vec![parent(map_id())],
        120,
    ));

    assert_eq!(
        Interface::<Bare>::find_by_id_raw(entry_id()).as_deref(),
        Some(&b"v2"[..])
    );
    assert_eq!(root_hash::<Bare>(), root_hash::<Named>());
}

/// The parent is what relinks an entry a newer update brings back from a
/// concurrent delete, so the one this node stores has to do that too.
#[test]
fn an_update_naming_no_parent_relinks_a_tombstone_it_outlives() {
    setup();
    let delete = Action::DeleteRef {
        id: entry_id(),
        deleted_at: 115,
        metadata: Metadata::new(110, 115),
    };
    apply::<Bare>(delete.clone());
    apply::<Named>(delete);
    assert!(ChildTrie::<Bare>::new(map_id()).get(entry_id()).is_none());

    apply::<Bare>(upsert(false, entry_id(), b"v2", vec![], 120));
    apply::<Named>(upsert(
        false,
        entry_id(),
        b"v2",
        vec![parent(map_id())],
        120,
    ));

    assert!(
        ChildTrie::<Bare>::new(map_id()).get(entry_id()).is_some(),
        "the entry is live again, so its parent lists it"
    );
    assert_eq!(root_hash::<Bare>(), root_hash::<Named>());
}

fn hlc(ns: u64) -> HybridTimestamp {
    HybridTimestamp::new(Timestamp::new(
        NTP64(ns),
        ID::from(core::num::NonZeroU64::new(1).unwrap()),
    ))
}

fn replay<S: StorageAdaptor>(actions: Vec<Action>) {
    let payload = to_vec(&StorageDelta::CausalActions {
        actions,
        delta_id: [0xD1; 32],
        delta_hlc: hlc(300),
        effective_writers: Default::default(),
        signer_account: None,
    })
    .unwrap();
    Root::<EmptyData, S>::sync(&payload, &ApplyContext::empty())
        .expect("an update this node cannot place must not fail the delta");
}

/// A node that never got the entry (its writer had it from entity-level sync,
/// or this node collected a concurrent delete's tombstone) cannot place an
/// update that names no parent. It drops it, keeps the rest of the delta, and
/// leaves the divergence to entity-level sync.
#[test]
fn a_delta_drops_an_update_to_an_entry_this_node_lacks() {
    setup();
    let unknown = Id::new([0x33; 32]);

    replay::<Bare>(vec![
        upsert(false, unknown, b"lost", vec![], 120),
        upsert(false, entry_id(), b"v2", vec![], 130),
    ]);

    assert!(Index::<Bare>::get_index(unknown).unwrap().is_none());
    assert_eq!(
        Interface::<Bare>::find_by_id_raw(entry_id()).as_deref(),
        Some(&b"v2"[..]),
        "the rest of the delta applies"
    );
}

/// A tombstoned parent can be collected while an entry that outlived its
/// delete keeps pointing at it. Recreating the parent from nothing is not an
/// option without its record, so the update is dropped.
#[test]
fn a_delta_drops_an_update_whose_stored_parent_was_collected() {
    setup();
    let _removed = Bare::storage_remove(crate::store::Key::Index(map_id()));

    replay::<Bare>(vec![upsert(false, entry_id(), b"v2", vec![], 120)]);

    assert_eq!(
        Interface::<Bare>::find_by_id_raw(entry_id()).as_deref(),
        Some(&b"v1"[..])
    );
}
