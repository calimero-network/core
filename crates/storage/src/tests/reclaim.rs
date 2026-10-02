//! Tombstone GC over physical entity rows: what a delete leaves behind, what
//! GC reclaims, and that dropping a collected child from its parent's
//! `deleted_children` changes nothing a replica can observe.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use super::*;
use crate::collections::{Root, UnorderedMap};
use crate::delta::{clear_pending_delta, StorageDelta};
use crate::entities::{EntryRules, StorageType};
use crate::env::{take_last_artifact, with_runtime_env, RuntimeEnv};
use crate::index::{EntityIndex, Index};
use crate::interface::{ApplyContext, Interface};
use crate::row::{encode, Row};
use crate::store::{Key, MainStorage, KEY_LEN};

type Rows = Rc<RefCell<BTreeMap<[u8; KEY_LEN], Vec<u8>>>>;
type Map = UnorderedMap<String, String, MainStorage>;

/// Shared by every replica, so they hold the same collection.
const FIELD: &str = "reclaimed";

/// Must be the native default: `ROOT_ID` is a process-global `LazyLock` seeded
/// from the first context id any test on the process reads.
const CONTEXT_ID: [u8; 32] = [236; 32];

fn env(rows: &Rows, device: [u8; 32]) -> RuntimeEnv {
    let r = Rc::clone(rows);
    let read = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
    let w = Rc::clone(rows);
    let write = Rc::new(move |key: Key, value: &[u8]| {
        w.borrow_mut()
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let rm = Rc::clone(rows);
    let remove = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
    RuntimeEnv::new(read, write, remove, CONTEXT_ID, device, [3; 32])
}

/// Runs `f` against `rows` as replica `device`, with nothing pending from a
/// previous run.
fn on<R>(rows: &Rows, device: u8, f: impl FnOnce() -> R) -> R {
    clear_pending_delta();
    with_runtime_env(env(rows, [device; 32]), f)
}

/// One node GC sweep over `rows` once every member has caught up past every
/// tombstone, as `calimero-node`'s does it then: the tombstones go, then, when
/// `prune` is set, every parent drops the children whose rows are gone.
/// `prune: false` is GC as it was before.
fn gc_pass(rows: &Rows, prune: bool) {
    let entity = |key: &[u8; KEY_LEN]| match Key::from_bytes(key) {
        Some(Key::Index(id)) => Some(id),
        _ => None,
    };
    let mut rows = rows.borrow_mut();
    rows.retain(|key, value| {
        entity(key).is_none_or(|id| tombstone_deleted_at(id, value).is_none())
    });
    if !prune {
        return;
    }
    let pruned: Vec<_> = rows
        .iter()
        .filter_map(|(key, value)| {
            let pruned = prune_deleted_children(entity(key)?, value, |child| {
                rows.contains_key(&Key::Index(child).to_bytes())
            })?;
            Some((*key, pruned))
        })
        .collect();
    rows.extend(pruned);
}

fn resident(rows: &Rows) -> (usize, usize) {
    let rows = rows.borrow();
    (rows.len(), rows.values().map(Vec::len).sum())
}

fn copy(rows: &Rows) -> Rows {
    Rc::new(RefCell::new(rows.borrow().clone()))
}

#[test]
fn deleted_entries_cost_nothing_once_collected() {
    let rows: Rows = Rc::default();
    // A live entry, so the map has been written before the measured deletes
    // and its own row is not about to grow by the first `updated_at` it
    // records.
    on(&rows, 1, || {
        let mut map = Root::new(|| Map::new_with_field_name(FIELD));
        drop(map.insert("kept".to_owned(), "kept".to_owned()).unwrap());
        map.commit();
    });
    let before = resident(&rows);

    let (entries, parent) = on(&rows, 1, || {
        let mut map = Root::<Map>::fetch().unwrap();
        let entries: Vec<_> = (0..100)
            .map(|i| {
                let key = format!("key-{i}");
                drop(map.insert(key.clone(), format!("value-{i}")).unwrap());
                map.entry_id(&key)
            })
            .collect();
        let parent = Index::<MainStorage>::get_parent_id(entries[0])
            .unwrap()
            .unwrap();
        map.commit();
        (entries, parent)
    });
    on(&rows, 1, || {
        let mut map = Root::<Map>::fetch().unwrap();
        for i in 0..100 {
            drop(map.remove(&format!("key-{i}")).unwrap());
        }
        map.commit();
    });

    // What a delete leaves: one tombstone row per entry, and every entry's id
    // in the map's own row.
    let deleted = resident(&rows);
    assert_eq!(deleted.0, before.0 + entries.len());
    let listed = |rows: &Rows| {
        on(rows, 1, || {
            Index::<MainStorage>::get_index(parent)
                .unwrap()
                .unwrap()
                .deleted_children()
                .len()
        })
    };
    assert_eq!(listed(&rows), entries.len());

    // GC as it was: the tombstones go, their ids stay in the map, 32 bytes each.
    let tombstones_only = copy(&rows);
    gc_pass(&tombstones_only, false);
    let leaked = resident(&tombstones_only);
    assert_eq!(leaked.0, before.0);
    assert_eq!(leaked.1, before.1 + 32 * entries.len() + 2);
    assert_eq!(listed(&tombstones_only), entries.len());

    // With pruning, nothing of the deleted entries is left.
    gc_pass(&rows, true);
    assert_eq!(resident(&rows), before);
    assert_eq!(listed(&rows), 0);
}

/// A write that is concurrent with a delete, and older than it, reaches the
/// deleting replica late. While the tombstone is kept it makes the delete win.
/// Once GC has collected the tombstone, the write is applied as a new entry,
/// which is why the node collects a tombstone only once every member has
/// caught up past it, the writer of any such write included. Dropping the
/// collected id from the parent too must not change any of it: both
/// replicas end up byte for byte the same as with tombstone GC alone.
#[test]
fn a_late_write_behaves_the_same_with_the_parent_pruned() {
    // Replica 2 writes "k" first, so its write is older than replica 1's
    // delete; its delta is held back.
    let late: Rows = Rc::default();
    let late_write = on(&late, 2, || {
        let mut map = Root::new(|| Map::new_with_field_name(FIELD));
        drop(map.insert("k".to_owned(), "late".to_owned()).unwrap());
        map.commit();
        take_last_artifact().unwrap()
    });
    let apply_late = |rows: &Rows| {
        let actions = match borsh::from_slice::<StorageDelta>(&late_write).unwrap() {
            StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
        };
        on(rows, 1, || {
            for action in actions.into_iter().filter(|action| !action.id().is_root()) {
                Interface::<MainStorage>::apply_action(action, &ApplyContext::empty()).unwrap();
            }
        });
    };
    let read = |rows: &Rows| {
        on(rows, 1, || {
            Root::<Map>::fetch()
                .unwrap()
                .get("k")
                .unwrap()
                .map(|value| value.clone())
        })
    };

    let rows: Rows = Rc::default();
    on(&rows, 1, || {
        let mut map = Root::new(|| Map::new_with_field_name(FIELD));
        drop(map.insert("k".to_owned(), "deleted".to_owned()).unwrap());
        map.commit();
    });
    on(&rows, 1, || {
        let mut map = Root::<Map>::fetch().unwrap();
        drop(map.remove("k").unwrap());
        map.commit();
    });

    let kept = copy(&rows);
    apply_late(&kept);
    assert_eq!(
        read(&kept),
        None,
        "delete must win while the tombstone is kept"
    );

    let tombstones_only = copy(&rows);
    gc_pass(&tombstones_only, false);
    let pruned = copy(&rows);
    gc_pass(&pruned, true);
    assert_ne!(*tombstones_only.borrow(), *pruned.borrow());

    apply_late(&tombstones_only);
    apply_late(&pruned);
    assert_eq!(read(&pruned), Some("late".to_owned()));
    assert_eq!(
        *tombstones_only.borrow(),
        *pruned.borrow(),
        "pruning the parent changed how a late write lands"
    );
}

fn tombstone(id: Id, deleted_at: u64) -> EntityIndex {
    let mut index = EntityIndex::minimal_for_test(id);
    index.deleted_at = Some(deleted_at);
    index
}

fn row_of(index: &EntityIndex, data: Option<&[u8]>) -> Vec<u8> {
    encode(
        index.id(),
        &Row {
            index: Some(borsh::to_vec(index).unwrap()),
            data: data.map(<[u8]>::to_vec),
        },
    )
}

#[test]
fn only_a_non_terminal_tombstone_is_collectable() {
    let id = Id::new([1; 32]);
    let row = row_of(&tombstone(id, 100), None);
    assert_eq!(tombstone_deleted_at(id, &row), Some(100));

    let live = row_of(&EntityIndex::minimal_for_test(id), Some(b"data"));
    assert_eq!(tombstone_deleted_at(id, &live), None);

    let mut written_once = tombstone(id, 100);
    written_once.metadata.storage_type = StorageType::User {
        owner: [7; 32].into(),
        rules: EntryRules {
            immutable: true,
            moderators: None,
        },
        signature_data: None,
    };
    assert_eq!(tombstone_deleted_at(id, &row_of(&written_once, None)), None);
}

#[test]
fn pruning_drops_only_children_without_a_row() {
    let parent = Id::new([9; 32]);
    let (gone, kept) = (Id::new([1; 32]), Id::new([2; 32]));
    let mut index = EntityIndex::minimal_for_test(parent);
    index.deleted_children = vec![gone, kept];
    let row = row_of(&index, Some(b"collection"));
    assert!(lists_deleted_children(parent, &row));

    let pruned = prune_deleted_children(parent, &row, |child| child == kept).unwrap();
    index.deleted_children = vec![kept];
    assert_eq!(pruned, row_of(&index, Some(b"collection")));

    assert_eq!(prune_deleted_children(parent, &pruned, |_| true), None);
    let emptied = prune_deleted_children(parent, &pruned, |_| false).unwrap();
    assert!(!lists_deleted_children(parent, &emptied));
    assert_eq!(prune_deleted_children(parent, &emptied, |_| false), None);

    // Bytes that are not an entity row this layer wrote are left alone.
    assert_eq!(prune_deleted_children(parent, b"junk", |_| false), None);
}
