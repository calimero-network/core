//! What one write costs its ancestors.
//!
//! A write that moves an entity's hash walks up to the root, storing each hash
//! in its parent's child trie. The walk used to look the child up, read its
//! index row for metadata it already held, write the trie, read the trie's root
//! back and count the root's children for a log line, at every level, and it
//! never stopped: a level whose slot already held the child's hash rewrote every
//! row above it with the bytes it held. An insert into a map paid for that
//! twice, once for the link and once for the value write that follows it with
//! the same bytes.
//!
//! A peer's delta applies its actions with the walks deferred to the end, but
//! each applied entry also relinked itself under its parent at once, and the
//! deferred walks then ran one per dirty entity: a delta updating `k` entries
//! of a map rewrote the map's row and its trie spine `k` times each.
//!
//! These pin the walk to one descent per trie and one write per ancestor whose
//! hash moved, and the tree to the root a store built directly would hold.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use calimero_storage::address::Id;
use calimero_storage::collections::{Root, UnorderedMap};
use calimero_storage::env::{take_last_artifact, with_runtime_env, RuntimeEnv};
use calimero_storage::index::Index;
use calimero_storage::interface::ApplyContext;
use calimero_storage::store::{Key, MainStorage, KEY_LEN};

type Map = UnorderedMap<String, String, MainStorage>;

const ENTRIES: usize = 1_000;

#[derive(Default)]
struct Backing {
    state: BTreeMap<[u8; KEY_LEN], Vec<u8>>,
    reads: BTreeMap<[u8; KEY_LEN], usize>,
    writes: BTreeMap<[u8; KEY_LEN], usize>,
}

impl Backing {
    fn reset(&mut self) {
        self.reads.clear();
        self.writes.clear();
    }

    fn total(counts: &BTreeMap<[u8; KEY_LEN], usize>) -> usize {
        counts.values().sum()
    }

    /// How often each child-trie row was read, the most-read first.
    fn trie_reads(&self) -> Vec<usize> {
        let mut reads: Vec<usize> = self
            .reads
            .iter()
            .filter(|(key, _)| matches!(Key::from_bytes(&key[..]), Some(Key::ChildTrie(_))))
            .map(|(_, n)| *n)
            .collect();
        reads.sort_unstable_by(|a, b| b.cmp(a));
        reads
    }

    fn reads_of(&self, key: Key) -> usize {
        self.reads.get(&key.to_bytes()).copied().unwrap_or(0)
    }

    fn writes_of(&self, key: Key) -> usize {
        self.writes.get(&key.to_bytes()).copied().unwrap_or(0)
    }
}

type Shared = Rc<RefCell<Backing>>;

fn env(backing: &Shared) -> RuntimeEnv {
    env_of(backing, [2; 32])
}

fn env_of(backing: &Shared, device: [u8; 32]) -> RuntimeEnv {
    let b = Rc::clone(backing);
    let read = Rc::new(move |key: &Key| {
        let mut b = b.borrow_mut();
        *b.reads.entry(key.to_bytes()).or_default() += 1;
        b.state.get(&key.to_bytes()).cloned()
    });
    let b = Rc::clone(backing);
    let write = Rc::new(move |key: Key, value: &[u8]| {
        let mut b = b.borrow_mut();
        *b.writes.entry(key.to_bytes()).or_default() += 1;
        b.state.insert(key.to_bytes(), value.to_vec()).is_some()
    });
    let b = Rc::clone(backing);
    let remove = Rc::new(move |key: &Key| b.borrow_mut().state.remove(&key.to_bytes()).is_some());
    RuntimeEnv::new(read, write, remove, [1; 32], device, [3; 32])
}

fn build(entries: impl IntoIterator<Item = (String, String)>) -> Root<Map> {
    let mut map = Root::new(|| Map::new_with_field_name("walk"));
    for (key, value) in entries {
        let _previous = map.insert(key, value).expect("insert should succeed");
    }
    map
}

fn filled() -> impl Iterator<Item = (String, String)> {
    (0..ENTRIES).map(|i| (format!("key{i}"), format!("value{i}")))
}

/// The context root's id and hash. Read inside the runtime environment, since
/// the root's id is the context's.
fn root() -> (Id, [u8; 32]) {
    let hash = Index::<MainStorage>::get_hashes_for(Id::root())
        .expect("the root index should read")
        .expect("the root should exist")
        .0;
    (Id::root(), hash)
}

/// Runs `f` against `backing`, a store holding a map of [`ENTRIES`] entries,
/// counting only what `f` does. Returns the context root after.
fn after_filled(backing: &Shared, f: impl FnOnce(&mut Root<Map>)) -> (Id, [u8; 32]) {
    with_runtime_env(env(backing), || {
        let mut map = build(filled());
        backing.borrow_mut().reset();
        f(&mut map);
        root()
    })
}

/// The root hash of a store holding exactly `entries`, built from nothing.
fn root_of(entries: impl IntoIterator<Item = (String, String)>) -> [u8; 32] {
    let backing = Shared::default();
    with_runtime_env(env(&backing), || {
        let _map = build(entries);
        root().1
    })
}

#[test]
fn an_update_descends_each_trie_once_and_writes_each_ancestor_once() {
    let backing = Shared::default();
    let (root_id, root_hash) = after_filled(&backing, |map| {
        let _previous = map
            .insert("key5".to_owned(), "changed".to_owned())
            .expect("update should succeed");
    });
    let b = backing.borrow();

    // The map's trie is three rows deep at this size and the context root's
    // one. Before, the walk read each of them two to four times: a lookup, the
    // replacement's own descent, the root read back, a count for a log line.
    assert_eq!(
        b.trie_reads().first().copied(),
        Some(1),
        "a child-trie row was read more than once: {:?}",
        b.trie_reads()
    );
    assert_eq!(b.writes_of(Key::Index(root_id)), 1, "the root's row");
    // The entry, three map-trie rows, the map, the root-trie row, the root.
    assert_eq!(Backing::total(&b.writes), 7, "rows written by one update");

    let expected = root_of(filled().map(|(key, value)| {
        let value = if key == "key5" {
            "changed".to_owned()
        } else {
            value
        };
        (key, value)
    }));
    assert_eq!(
        root_hash, expected,
        "the walk left a stale hash on the way up"
    );
}

#[test]
fn an_insert_walks_to_the_root_once() {
    let backing = Shared::default();
    let (root_id, root_hash) = after_filled(&backing, |map| {
        let _previous = map
            .insert("new".to_owned(), "value".to_owned())
            .expect("insert should succeed");
    });
    let b = backing.borrow();

    // The link walks from the map to the root. The value write after it stores
    // the bytes the link wrote, which moves no hash, so it walks nowhere:
    // before, it walked to the root again and rewrote every row on the way.
    assert_eq!(b.writes_of(Key::Index(root_id)), 1, "the root's row");
    // Once for the call itself and once by the one walk that reaches it.
    assert_eq!(
        b.reads_of(Key::Index(root_id)),
        2,
        "reads of the root's row"
    );

    let expected = root_of(filled().chain([("new".to_owned(), "value".to_owned())]));
    assert_eq!(
        root_hash, expected,
        "the walk left a stale hash on the way up"
    );
}

#[test]
fn a_write_of_the_bytes_already_stored_moves_no_ancestor() {
    let backing = Shared::default();
    let (root_id, root_hash) = after_filled(&backing, |map| {
        let _previous = map
            .insert("key5".to_owned(), "value5".to_owned())
            .expect("rewrite should succeed");
    });
    let b = backing.borrow();

    // The entry's own row alone: its stamp moved, its hash did not.
    assert_eq!(Backing::total(&b.writes), 1, "rows written");
    // Once for the call itself; no walk reaches it.
    assert_eq!(
        b.reads_of(Key::Index(root_id)),
        1,
        "reads of the root's row"
    );
    assert_eq!(root_hash, root_of(filled()), "the root hash moved");
}

/// A peer's delta updating the first `updated` of `ENTRIES` entries, applied
/// through `Root::sync` as `__calimero_sync_next` applies it: the most writes
/// any one row took, and whether the receiver's root matches the sender's.
fn apply_update_delta(updated: usize) -> (usize, bool) {
    let sender = Shared::default();
    let (create, update, sender_root) = with_runtime_env(env_of(&sender, [9; 32]), || {
        build(filled()).commit();
        let create = take_last_artifact().expect("the build should emit a delta");
        let mut map = Root::<Map>::fetch().expect("the map was just committed");
        for i in 0..updated {
            let _previous = map
                .insert(format!("key{i}"), "changed".to_owned())
                .expect("update should succeed");
        }
        map.commit();
        let update = take_last_artifact().expect("the updates should emit a delta");
        (create, update, root().1)
    });

    let receiver = Shared::default();
    with_runtime_env(env(&receiver), || {
        Root::<Map>::sync(&create, &ApplyContext::empty()).expect("the build should apply");
        receiver.borrow_mut().reset();
        Root::<Map>::sync(&update, &ApplyContext::empty()).expect("the updates should apply");
        let hottest = receiver
            .borrow()
            .writes
            .values()
            .copied()
            .max()
            .unwrap_or(0);
        (hottest, root().1 == sender_root)
    })
}

#[test]
fn a_delta_rewrites_each_ancestor_once_however_many_entries_it_updates() {
    for updated in [1, 64] {
        let (hottest, converged) = apply_update_delta(updated);
        assert!(
            converged,
            "{updated} updates: the receiver's root is not the sender's"
        );
        // Each entry's row, each trie row and each ancestor's row, once. Each
        // updated entry used to relink itself under the map, which wrote its
        // row a second time and the map's row and every row of the map's trie
        // spine once per entry.
        assert_eq!(
            hottest, 1,
            "{updated} updates: the most writes any one row took"
        );
    }
}
