//! Tombstone GC over physical entity rows: what a delete leaves behind, what
//! GC reclaims, and that dropping a collected child from its parent's
//! `deleted_children` changes nothing a replica can observe.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use calimero_account::AccountId;
use ed25519_dalek::SigningKey;

use super::*;
use crate::action::Action;
use crate::collections::{cell_value_id, Authored, Root, UnorderedMap};
use crate::delta::{clear_pending_delta, StorageDelta};
use crate::entities::{ChildInfo, Data, EntryRules, Metadata, StorageType};
use crate::env::{take_last_artifact, with_runtime_env, RuntimeEnv};
use crate::index::{EntityIndex, Index};
use crate::interface::{ApplyContext, Interface, StorageError};
use crate::logical_clock::HybridTimestamp;
use crate::row::{encode, Row};
use crate::store::{Key, MainStorage, KEY_LEN};
use crate::tests::common::{
    account_of_key, apply_ctx_for, build_signed_member_action, build_signed_member_delete,
    build_signed_shared_action, build_signed_shared_delete, cell_at, map_entry_bytes,
    setup_root_for_main, writers_of,
};
use crate::tests::owned_rules::{delete, key, signed};

type Rows = Rc<RefCell<BTreeMap<[u8; KEY_LEN], Vec<u8>>>>;
type Map = UnorderedMap<String, String, MainStorage>;
type Notes = Authored<UnorderedMap<String, String>>;

/// Shared by every replica, so they hold the same collection.
const FIELD: &str = "reclaimed";

/// The owned collection's field name and deterministic id seed.
const NOTES: &str = "notes";

/// The key of the owned entry the replay tests write and delete.
const ENTRY_KEY: &str = "k";

/// When the owner's first write was made; a fixed past instant.
const WRITTEN_AT: u64 = 1_700_000_000_000_000_000;

/// When the owner deleted that entry.
const DELETED_AT: u64 = WRITTEN_AT + 1;

/// Between a cell test's writes, so its last (step 6) stays within the
/// future-drift tolerance of the clock it starts from.
const CELL_STEP_NANOS: u64 = 500_000_000;

/// How far behind a collected delete a re-inserting owner's clock reads.
const CLOCK_LAG_NANOS: u64 = 60_000_000_000;

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
/// tombstone, as `calimero-node`'s does it then: each tombstone goes, a signed
/// entity's leaving the record of its delete, then, when `prune` is set, every
/// parent drops the children whose rows are gone. `prune: false` is GC as it
/// was before.
fn gc_pass(rows: &Rows, prune: bool) {
    let entity = |key: &[u8; KEY_LEN]| match Key::from_bytes(key) {
        Some(Key::Index(id)) => Some(id),
        _ => None,
    };
    let mut rows = rows.borrow_mut();
    let collected: Vec<_> = rows
        .iter()
        .filter_map(|(key, value)| {
            let id = entity(key)?;
            let _deleted_at = tombstone_deleted_at(id, value)?;
            Some((id, deleted_at_to_record(id, value)))
        })
        .collect();
    for (id, deleted_at) in collected {
        let _tombstone = rows.remove(&Key::Index(id).to_bytes());
        if let Some(deleted_at) = deleted_at {
            let record_key = Key::Collected(id).to_bytes();
            let record = collected_record(deleted_at, rows.get(&record_key).map(Vec::as_slice));
            let _previous = rows.insert(record_key, record.to_vec());
        }
    }
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

/// An owner's entry that a peer's signed actions wrote and deleted, and whose
/// tombstone GC has since collected.
struct CollectedEntry {
    rows: Rows,
    owner_key: SigningKey,
    owner: AccountId,
    parent: Id,
    id: Id,
    rules: EntryRules,
    /// The first write's bytes, as a peer that relayed it still holds them.
    original: Vec<u8>,
}

impl CollectedEntry {
    fn new() -> Self {
        let owner_key = key(0xA1);
        let owner = account_of_key(&owner_key);
        let rows: Rows = Rc::default();
        let (parent, id, rules) = on(&rows, 1, || {
            let notes = Root::new(|| {
                let mut notes = Notes::new_with_field_name(NOTES);
                notes.reassign_deterministic_id(NOTES);
                notes
            });
            let inner: &UnorderedMap<String, String> = &notes;
            let parent = inner.id();
            let id = notes.entry_id_of(&owner, &ENTRY_KEY.to_owned());
            let rules = notes.entry_rules();
            notes.commit();
            (parent, id, rules)
        });
        let mut entry = Self {
            rows,
            owner_key,
            owner,
            parent,
            id,
            rules,
            original: Vec::new(),
        };
        entry.original = borsh::to_vec(&entry.write("deleted", WRITTEN_AT)).unwrap();
        entry
            .apply(borsh::from_slice(&entry.original).unwrap())
            .unwrap();
        assert_eq!(entry.read(), Some("deleted".to_owned()));

        let removal = signed(
            delete(id, DELETED_AT),
            owner,
            rules,
            &entry.owner_key,
            DELETED_AT,
        );
        entry.apply(removal).unwrap();
        assert_eq!(entry.read(), None);

        gc_pass(&entry.rows, true);
        let collected = on(&entry.rows, 1, || {
            Index::<MainStorage>::get_index(id).unwrap()
        });
        assert!(collected.is_none(), "GC left the tombstone in place");
        entry
    }

    /// The owner's signed write of `value` at `at`.
    fn write(&self, value: &str, at: u64) -> Action {
        let (id, parent) = (self.id, self.parent);
        let data = map_entry_bytes(id, &ENTRY_KEY.to_owned(), &value.to_owned());
        let add = move |metadata| Action::Add {
            id,
            data,
            ancestors: vec![ChildInfo::new(parent, [0; 32], Metadata::default())],
            metadata,
        };
        signed(add, self.owner, self.rules, &self.owner_key, at)
    }

    fn apply(&self, action: Action) -> Result<(), StorageError> {
        on(&self.rows, 1, || {
            Interface::<MainStorage>::apply_action(action, &apply_ctx_for(self.owner))
        })
    }

    fn read(&self) -> Option<String> {
        on(&self.rows, 1, || {
            Root::<Notes>::fetch()
                .unwrap()
                .get_by(&self.owner, &ENTRY_KEY.to_owned())
                .unwrap()
        })
    }
}

/// A peer replaying the owner's original signed write must not bring a
/// collected entry back, while a later owner write lands.
#[test]
fn a_replayed_signed_write_does_not_bring_back_a_collected_entry() {
    let entry = CollectedEntry::new();

    // Dropped as stale, as the tombstone would have it, not refused as an error.
    entry
        .apply(borsh::from_slice(&entry.original).unwrap())
        .unwrap();
    assert_eq!(
        entry.read(),
        None,
        "a replayed write brought back a deleted entry"
    );

    entry
        .apply(entry.write("rewritten", DELETED_AT + 1))
        .unwrap();
    assert_eq!(entry.read(), Some("rewritten".to_owned()));
}

/// The delete wins a tie, collected or not: a write stamped with the delete's
/// own time stays deleted.
#[test]
fn a_write_stamped_at_a_collected_delete_stays_deleted() {
    let entry = CollectedEntry::new();
    entry.apply(entry.write("tied", DELETED_AT)).unwrap();
    assert_eq!(
        entry.read(),
        None,
        "a write no newer than the collected delete brought it back"
    );
}

/// The owner re-inserts a key after GC collected its delete, on a clock behind
/// that delete: the write must still be stamped after it, or peers drop it.
#[test]
fn a_re_insert_after_collection_is_stamped_past_the_delete() {
    let rows: Rows = Rc::default();
    let entry_key = ENTRY_KEY.to_owned();
    let id = on(&rows, 1, || {
        let mut notes = Root::new(|| {
            let mut notes = Notes::new_with_field_name(NOTES);
            notes.reassign_deterministic_id(NOTES);
            notes
        });
        notes.insert(entry_key.clone(), "first".to_owned()).unwrap();
        let id = notes.entry_id(&entry_key);
        notes.commit();
        id
    });
    on(&rows, 1, || {
        let mut notes = Root::<Notes>::fetch().unwrap();
        drop(notes.remove(&entry_key).unwrap());
        notes.commit();
    });
    let deleted_at = on(&rows, 1, || {
        Index::<MainStorage>::get_index(id)
            .unwrap()
            .and_then(|index| index.deleted_at)
            .unwrap()
    });
    gc_pass(&rows, true);
    let collected = on(&rows, 1, || Index::<MainStorage>::get_index(id).unwrap());
    assert!(collected.is_none(), "GC left the tombstone in place");

    let nonce = on(&rows, 1, || {
        let previous = crate::env::begin_execution_for_testing(deleted_at - CLOCK_LAG_NANOS);
        let mut notes = Root::<Notes>::fetch().unwrap();
        notes.insert(entry_key.clone(), "again".to_owned()).unwrap();
        notes.commit();
        crate::env::restore_wall_clock_for_testing(previous);
        signed_nonce_in_last_delta(id)
    });
    assert!(
        nonce > deleted_at,
        "a re-insert on a lagging clock was signed at {nonce}, not after the collected delete at {deleted_at}"
    );
}

/// The nonce the last committed delta signs `id`'s write with.
fn signed_nonce_in_last_delta(id: Id) -> u64 {
    let delta = take_last_artifact().unwrap();
    let actions = match borsh::from_slice::<StorageDelta>(&delta).unwrap() {
        StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
    };
    actions
        .into_iter()
        .find_map(|action| match action {
            Action::Add {
                id: written,
                metadata,
                ..
            }
            | Action::Update {
                id: written,
                metadata,
                ..
            } if written == id => match metadata.storage_type {
                StorageType::User {
                    signature_data: Some(signature),
                    ..
                } => Some(signature.nonce),
                _ => None,
            },
            _ => None,
        })
        .unwrap()
}

/// Which entity a cell test deletes, has GC collect, and replays.
enum Target {
    Cell,
    CellEntry,
    /// The cell, after a peer first names it as an ancestor under an unsigned
    /// stamp, so it is stored again before its write is replayed.
    CellNamedAsAncestor,
}

/// Deletes a writer's `target`, has GC collect it, replays its original write,
/// then writes it afresh: what is stored after the replay, and after that write.
fn replay_after_collection(target: Target) -> (Option<EntityIndex>, Option<EntityIndex>) {
    let writer_key = key(0xB1);
    let writer = account_of_key(&writer_key);
    let writers: BTreeSet<AccountId> = [writer].into_iter().collect();
    let cell = cell_at(0xB1, &writers);
    let anchor = cell_at(0xB2, &writers);
    let entry = cell_value_id(anchor);
    // Stamped from the clock, which also stamps a cell's rotation-log entry: a
    // delete older than that entry would keep it, and the cell, alive.
    let start = crate::env::time_now();
    let at = |step: u8| start + u64::from(step) * CELL_STEP_NANOS;

    let rows: Rows = Rc::default();
    let apply = |action: Action, delta: u8| {
        let ctx = ApplyContext {
            effective_writers: None,
            delta_id: Some([delta; 32]),
            delta_hlc: Some(HybridTimestamp::from_unix_nanos(at(delta))),
            signer_account: Some(writer),
        };
        on(&rows, 1, || {
            Interface::<MainStorage>::apply_action(action, &ctx)
        })
    };
    let stored = |id: Id| on(&rows, 1, || Index::<MainStorage>::get_index(id).unwrap());
    let root = on(&rows, 1, setup_root_for_main);

    let shared_write = |id: Id, value: &[u8], step: u8| {
        build_signed_shared_action(
            true,
            id,
            value.to_vec(),
            writers.clone(),
            at(step),
            &writer_key,
            vec![root.clone()],
        )
    };
    let entry_write = |value: &[u8], step: u8| {
        let ancestors = vec![
            ChildInfo::new(anchor, [0; 32], Metadata::default()),
            root.clone(),
        ];
        build_signed_member_action(
            true,
            entry,
            anchor,
            value.to_vec(),
            at(step),
            &writer_key,
            ancestors,
        )
    };
    apply(shared_write(anchor, b"anchor", 1), 1).unwrap();
    apply(shared_write(cell, b"cell", 2), 2).unwrap();
    apply(entry_write(b"entry", 3), 3).unwrap();

    let (id, original, delete, fresh) = match target {
        Target::Cell | Target::CellNamedAsAncestor => (
            cell,
            shared_write(cell, b"cell", 2),
            build_signed_shared_delete(cell, writers.clone(), &writer_key, at(4)),
            shared_write(cell, b"cell again", 6),
        ),
        Target::CellEntry => (
            entry,
            entry_write(b"entry", 3),
            build_signed_member_delete(entry, anchor, &writer_key, at(4)),
            entry_write(b"entry again", 6),
        ),
    };
    apply(delete, 4).unwrap();
    gc_pass(&rows, true);
    assert!(stored(id).is_none(), "GC left the tombstone in place");

    if matches!(target, Target::CellNamedAsAncestor) {
        let claimed = Metadata {
            updated_at: 1.into(),
            storage_type: StorageType::Shared {
                writers: writers_of(writers.iter().copied()),
                signature_data: None,
            },
            ..Metadata::default()
        };
        let leaf = Action::Add {
            id: Id::new([0x77; 32]),
            data: b"leaf".to_vec(),
            ancestors: vec![ChildInfo::new(cell, [0; 32], claimed), root.clone()],
            metadata: Metadata::default(),
        };
        let _named = apply(leaf, 7);
    }

    // Dropped as stale, as the tombstone would have it, not refused as an error.
    apply(original, 5).unwrap();
    let replayed = stored(id);
    apply(fresh, 6).unwrap();
    (replayed, stored(id))
}

/// A writer's replayed signed write does not bring back a collected cell.
#[test]
fn a_replayed_cell_write_does_not_bring_back_a_collected_cell() {
    let (replayed, rewritten) = replay_after_collection(Target::Cell);
    assert!(
        replayed.is_none(),
        "a replayed write brought back a deleted cell"
    );
    assert!(rewritten.is_some(), "a later write did not land");
}

/// A peer that names a collected cell as an ancestor cannot re-create it, and
/// so open it to a replayed write.
#[test]
fn a_collected_cell_named_as_an_ancestor_stays_gone() {
    let (replayed, rewritten) = replay_after_collection(Target::CellNamedAsAncestor);
    assert!(
        replayed.is_none(),
        "an ancestor stamp and a replayed write brought back a deleted cell"
    );
    assert!(rewritten.is_some(), "a later write did not land");
}

/// A writer's replayed signed write does not bring back a collected entry of
/// a cell.
#[test]
fn a_replayed_cell_entry_write_does_not_bring_back_a_collected_entry() {
    let (replayed, rewritten) = replay_after_collection(Target::CellEntry);
    assert!(
        replayed.is_none(),
        "a replayed write brought back a deleted cell entry"
    );
    assert!(rewritten.is_some(), "a later write did not land");
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
