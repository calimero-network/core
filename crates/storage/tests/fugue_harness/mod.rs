//! The storage harness shared by the `FugueText` integration tests: per-replica
//! stores reconciled by `Interface::apply_action`. A directory module, since
//! `tests/fugue_harness.rs` would be compiled as a test target of its own.
//! Each test binary uses a subset, so unused helpers here are not dead code.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::rc::Rc;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::{FugueText, Root};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::{Key, MainStorage};

pub type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

/// Must stay the native default: `ROOT_ID` is a process-global seeded from the
/// first `context_id()` read, so another value poisons `Root::new` process-wide.
const CONTEXT_ID: [u8; 32] = [236_u8; 32];

pub fn new_store() -> Store {
    Rc::new(RefCell::new(HashMap::new()))
}

pub fn fork(store: &Store) -> Store {
    Rc::new(RefCell::new(store.borrow().clone()))
}

pub fn env_for(store: &Store, device: [u8; 32]) -> RuntimeEnv {
    counting_env_for(store, device, &Rc::new(Cell::new(0)))
}

/// [`env_for`] with a tally of the host reads it issues.
pub fn counting_env_for(store: &Store, device: [u8; 32], reads: &Rc<Cell<usize>>) -> RuntimeEnv {
    let r = Rc::clone(store);
    let tally = Rc::clone(reads);
    let reader = Rc::new(move |key: &Key| {
        tally.set(tally.get() + 1);
        r.borrow().get(&key.to_bytes()).cloned()
    });
    let w = Rc::clone(store);
    let writer = Rc::new(move |key: Key, value: &[u8]| {
        w.borrow_mut()
            .insert(key.to_bytes(), value.to_vec())
            .is_some()
    });
    let rm = Rc::clone(store);
    let remover = Rc::new(move |key: &Key| rm.borrow_mut().remove(&key.to_bytes()).is_some());
    let mut account = device;
    account[1] = 0xAC;
    RuntimeEnv::new(reader, writer, remover, CONTEXT_ID, device, account)
}

/// Distinct in the first 8 bytes, which is all `local_replica` reads.
pub fn device(n: u8) -> [u8; 32] {
    let mut id = [n; 32];
    id[..8].copy_from_slice(&u64::from(n).to_be_bytes());
    id
}

/// Runs `f` against `store`'s document, returning the delta the commit emits.
pub fn edit<T: BorshSerialize + BorshDeserialize>(
    store: &Store,
    device: [u8; 32],
    f: impl FnOnce(&mut Root<T>),
) -> Vec<u8> {
    clear_pending_delta();
    env::with_runtime_env(env_for(store, device), || {
        let mut doc = Root::<T>::fetch().expect("document root should exist");
        f(&mut doc);
        doc.commit();
        env::take_last_artifact().expect("commit should emit a delta")
    })
}

/// Lands `delta` the way the sync path does, through `Interface::apply_action`.
pub fn land(store: &Store, device: [u8; 32], delta: &[u8]) {
    let actions = match borsh::from_slice::<StorageDelta>(delta).expect("delta should decode") {
        StorageDelta::Actions(actions) => actions,
        StorageDelta::CausalActions { actions, .. } => actions,
    };
    env::with_runtime_env(env_for(store, device), || {
        for action in actions {
            if action.id().is_root() {
                continue;
            }
            Interface::<MainStorage>::apply_action(action, &ApplyContext::empty())
                .expect("remote apply_action should succeed");
        }
    });
}

pub fn read_with<T: BorshSerialize + BorshDeserialize, R>(
    store: &Store,
    device: [u8; 32],
    read: impl FnOnce(&Root<T>) -> R,
) -> R {
    env::with_runtime_env(env_for(store, device), || {
        read(&Root::<T>::fetch().expect("document root should exist"))
    })
}

pub fn fugue_text_in(store: &Store, device: [u8; 32]) -> String {
    read_with::<FugueText<MainStorage>, _>(store, device, |doc| {
        doc.get_text().expect("get_text should succeed")
    })
}

/// A store holding one committed root state, built by `init` and seeded by `seed`.
pub fn genesis<T: BorshSerialize + BorshDeserialize>(
    init: impl FnOnce() -> T,
    seed: impl FnOnce(&mut Root<T>),
) -> Store {
    let store = new_store();
    clear_pending_delta();
    env::with_runtime_env(env_for(&store, device(1)), || {
        let mut root = Root::new(init);
        seed(&mut root);
        root.commit();
        let _ignored = env::take_last_artifact();
    });
    store
}

/// Authored under replica 0, so neither writer's counter space starts used.
pub fn fugue_genesis(field: &str, seed: &str) -> Store {
    genesis(
        || FugueText::<MainStorage>::new_with_field_name(field),
        |doc| {
            doc.insert_str_with_replica(0, 0, seed)
                .expect("seed insert should succeed");
        },
    )
}

/// The non-root entity ids a delta writes.
pub fn written_ids(delta: &[u8]) -> Vec<Id> {
    let actions = match borsh::from_slice::<StorageDelta>(delta).expect("delta should decode") {
        StorageDelta::Actions(actions) | StorageDelta::CausalActions { actions, .. } => actions,
    };
    actions
        .iter()
        .map(|action| action.id())
        .filter(|id| !id.is_root())
        .collect()
}

/// The stored bytes of every entity `ids` names, so two replicas can be compared row by row.
pub fn entry_bytes(store: &Store, ids: &BTreeSet<Id>) -> BTreeMap<Id, Option<Vec<u8>>> {
    ids.iter()
        .map(|id| {
            (
                *id,
                store.borrow().get(&Key::Entry(*id).to_bytes()).cloned(),
            )
        })
        .collect()
}
