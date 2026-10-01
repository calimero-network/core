//! A call on a guarded collection costs the same however large it has grown.
//!
//! A guarded collection (`AuthoredVector`, `Authored`/`WriteOnce`/`Moderated`
//! maps, `UserStorage`, anything nested in an owned entry or a cell) reads only
//! the entries its domain admits, so it cannot answer `len` from the child
//! trie's count, which includes entries it does not admit. It used to answer by
//! loading every child, so a chat contract that counted its messages on every
//! send spent gas linear in the channel's history and ran out of gas at a few
//! thousand messages. A keyed insert asked `contains` first, which loaded every
//! child too, guarded or not.
//!
//! Each workload here builds the collection, runs one call that counts it (the
//! first count after an upgrade walks the children once), then measures one
//! more call in a fresh execution, which is what a contract call is. The count
//! must not move with the collection's size.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{
    AuthoredMap, AuthoredVector, Root, UnorderedMap, Vector, WriteOnce,
};
use calimero_storage::env::{with_runtime_env, IndexCallbacks, RuntimeEnv};
use calimero_storage::store::{Key, MainStorage};

const SMALL: usize = 16;
const LARGE: usize = 1_024;

#[derive(Default)]
struct Backing {
    state: BTreeMap<[u8; calimero_storage::store::KEY_LEN], Vec<u8>>,
    index: BTreeMap<Vec<u8>, Vec<u8>>,
    meta: BTreeMap<Vec<u8>, Vec<u8>>,
    reads: usize,
}

type Shared = Rc<RefCell<Backing>>;

/// A runtime environment over `backing`, with the node-local index plane routed
/// to it too, so nothing leaks between runs through the thread-local mock.
fn env(backing: &Shared) -> RuntimeEnv {
    let b = Rc::clone(backing);
    let read = Rc::new(move |key: &Key| {
        let mut b = b.borrow_mut();
        b.reads += 1;
        b.state.get(&key.to_bytes()).cloned()
    });
    let b = Rc::clone(backing);
    let write = Rc::new(move |key: Key, value: &[u8]| {
        b.borrow_mut()
            .state
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let b = Rc::clone(backing);
    let remove = Rc::new(move |key: &Key| b.borrow_mut().state.remove(&key.to_bytes()).is_some());
    RuntimeEnv::new(read, write, remove, [1; 32], [2; 32], [3; 32]).with_index(index(backing))
}

fn index(backing: &Shared) -> IndexCallbacks {
    let b = Rc::clone(backing);
    let set = Rc::new(move |k: &[u8], v: &[u8]| {
        let _prev = b.borrow_mut().index.insert(k.to_vec(), v.to_vec());
        true
    });
    let b = Rc::clone(backing);
    let remove = Rc::new(move |k: &[u8]| b.borrow_mut().index.remove(k).is_some());
    let b = Rc::clone(backing);
    let remove_prefix = Rc::new(move |p: &[u8]| {
        b.borrow_mut().index.retain(|k, _| !k.starts_with(p));
        true
    });
    let b = Rc::clone(backing);
    let scan = Rc::new(
        move |lo: &[u8], hi: &[u8], offset: usize, limit: Option<usize>| {
            let b = b.borrow();
            let rows = b
                .index
                .range(lo.to_vec()..hi.to_vec())
                .skip(offset)
                .map(|(k, v)| (k.clone(), v.clone()));
            match limit {
                Some(n) => rows.take(n).collect(),
                None => rows.collect(),
            }
        },
    );
    let b = Rc::clone(backing);
    let last = Rc::new(move |lo: &[u8], hi: &[u8]| {
        b.borrow()
            .index
            .range(lo.to_vec()..hi.to_vec())
            .next_back()
            .map(|(k, v)| (k.clone(), v.clone()))
    });
    let b = Rc::clone(backing);
    let meta_set = Rc::new(move |k: &[u8], v: &[u8]| {
        let _prev = b.borrow_mut().meta.insert(k.to_vec(), v.to_vec());
        true
    });
    let b = Rc::clone(backing);
    let meta_get = Rc::new(move |k: &[u8]| b.borrow().meta.get(k).cloned());
    let b = Rc::clone(backing);
    let meta_clear = Rc::new(move |k: &[u8]| b.borrow_mut().meta.remove(k).is_some());
    IndexCallbacks {
        set,
        remove,
        remove_prefix,
        scan,
        last,
        meta_set,
        meta_get,
        meta_clear,
    }
}

/// State reads of `op`, run as a contract call against a collection of `n`
/// entries that an earlier call has already counted once.
fn reads_of_one_call<T>(
    n: usize,
    build: impl Fn(usize) -> T,
    count: impl Fn(&Root<T>),
    op: impl Fn(&mut Root<T>),
) -> usize
where
    T: BorshSerialize + BorshDeserialize,
{
    let backing = Shared::default();
    with_runtime_env(env(&backing), || {
        Root::new(|| build(n)).commit();

        let root = Root::<T>::fetch().expect("the root was just committed");
        count(&root);
        root.commit();

        let mut root = Root::<T>::fetch().expect("the root was just committed");
        backing.borrow_mut().reads = 0;
        op(&mut root);
        backing.borrow().reads
    })
}

/// Asserts `op` does not read the collection: between [`SMALL`] and [`LARGE`]
/// entries its reads may grow by at most a tenth of a row per entry added.
///
/// Not equality, because a debug build reconciles each child-trie node it
/// writes against the level below (`ChildTrie::debug_reconcile`), up to 16
/// rows a level, and a larger trie has fuller nodes. That is bounded however
/// large the trie grows. Loading the children costs about two rows per entry.
fn assert_flat<T>(
    what: &str,
    build: impl Fn(usize) -> T,
    count: impl Fn(&Root<T>),
    op: impl Fn(&mut Root<T>),
) where
    T: BorshSerialize + BorshDeserialize,
{
    let small = reads_of_one_call(SMALL, &build, &count, &op);
    let large = reads_of_one_call(LARGE, &build, &count, &op);
    assert!(
        large.saturating_sub(small) * 10 < LARGE - SMALL,
        "{what}: {small} reads at {SMALL} entries, {large} at {LARGE}: it reads the collection"
    );
}

fn authored_vector(n: usize) -> AuthoredVector<String, MainStorage> {
    let mut vector = AuthoredVector::new();
    for i in 0..n {
        let _id = vector.push(format!("message {i}")).expect("push");
    }
    vector
}

fn authored_map(n: usize) -> AuthoredMap<String, String, MainStorage> {
    let mut map = AuthoredMap::new();
    for i in 0..n {
        map.insert(format!("key {i}"), "value".to_owned())
            .expect("insert");
    }
    map
}

fn write_once_map(n: usize) -> WriteOnce<UnorderedMap<String, String, MainStorage>> {
    let mut map = WriteOnce::<UnorderedMap<String, String, MainStorage>>::new();
    for i in 0..n {
        map.insert(format!("key {i}"), "value".to_owned())
            .expect("insert");
    }
    map
}

fn unordered_map(n: usize) -> UnorderedMap<String, String, MainStorage> {
    let mut map = UnorderedMap::new();
    for i in 0..n {
        let _prev = map
            .insert(format!("key {i}"), "value".to_owned())
            .expect("insert");
    }
    map
}

fn vector(n: usize) -> Vector<String, MainStorage> {
    let mut vector = Vector::new();
    for i in 0..n {
        vector.push(format!("message {i}")).expect("push");
    }
    vector
}

#[test]
fn an_authored_vector_counts_and_appends_in_constant_reads() {
    let count = |v: &Root<AuthoredVector<String, MainStorage>>| {
        let _len = v.len().expect("len");
    };
    assert_flat("AuthoredVector::len", authored_vector, count, |v| {
        let _len = v.len().expect("len");
    });
    assert_flat("AuthoredVector::push", authored_vector, count, |v| {
        let _id = v.push("one more".to_owned()).expect("push");
    });
    // The contract's send: count, then append, then count what it appended.
    assert_flat(
        "AuthoredVector len + push + len",
        authored_vector,
        count,
        |v| {
            let before = v.len().expect("len");
            let _id = v.push("one more".to_owned()).expect("push");
            assert_eq!(v.len().expect("len"), before + 1);
        },
    );
}

#[test]
fn an_owned_map_counts_and_inserts_in_constant_reads() {
    let count = |m: &Root<AuthoredMap<String, String, MainStorage>>| {
        let _len = m.len().expect("len");
    };
    assert_flat("AuthoredMap::len", authored_map, count, |m| {
        let _len = m.len().expect("len");
    });
    assert_flat("AuthoredMap::insert", authored_map, count, |m| {
        m.insert("one more".to_owned(), "value".to_owned())
            .expect("insert");
    });
    let count = |m: &Root<WriteOnce<UnorderedMap<String, String, MainStorage>>>| {
        let _len = m.len().expect("len");
    };
    assert_flat("WriteOnce::insert", write_once_map, count, |m| {
        m.insert("one more".to_owned(), "value".to_owned())
            .expect("insert");
    });
}

#[test]
fn a_membership_check_reads_one_entry_not_the_collection() {
    let nothing = |_: &Root<UnorderedMap<String, String, MainStorage>>| {};
    assert_flat("UnorderedMap::contains", unordered_map, nothing, |m| {
        assert!(m.contains("key 3").expect("contains"));
        assert!(!m.contains("absent").expect("contains"));
    });
    assert_flat("UnorderedMap::entry", unordered_map, nothing, |m| {
        let _entry = m.entry("key 3".to_owned()).expect("entry");
    });
    assert_flat("UnorderedMap::remove", unordered_map, nothing, |m| {
        assert!(m.remove("key 3").expect("remove").is_some());
    });
    let nothing = |_: &Root<Vector<String, MainStorage>>| {};
    assert_flat("Vector::push + len", vector, nothing, |v| {
        v.push("one more".to_owned()).expect("push");
        let _len = v.len().expect("len");
    });
}
