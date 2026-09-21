//! The storage harness shared by the `FugueText` integration tests: two
//! replicas, separate stores, reconciled by `Interface::apply_action`.
//!
//! A directory module rather than `tests/fugue_harness.rs`, which cargo would
//! compile as a test target of its own.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{FugueText, Root};
use calimero_storage::delta::{clear_pending_delta, StorageDelta};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::interface::{ApplyContext, Interface};
use calimero_storage::store::{Key, MainStorage};

/// An in-memory main-storage backend owned by one replica.
pub type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

/// The NATIVE DEFAULT context. `collections::ROOT_ID` is a process-global
/// seeded from the first `context_id()` anything in the binary asks for, so a
/// test that installs another one poisons `Root::new` for the whole process.
const CONTEXT_ID: [u8; 32] = [236_u8; 32];

pub fn new_store() -> Store {
    Rc::new(RefCell::new(HashMap::new()))
}

/// A byte-for-byte copy of `store`: another replica bootstrapped from it.
pub fn fork(store: &Store) -> Store {
    Rc::new(RefCell::new(store.borrow().clone()))
}

/// A [`RuntimeEnv`] routing all `MainStorage` I/O into `store`, under `device`.
pub fn env_for(store: &Store, device: [u8; 32]) -> RuntimeEnv {
    let r = Rc::clone(store);
    let reader = Rc::new(move |key: &Key| r.borrow().get(&key.to_bytes()).cloned());
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

/// Device id of replica `n`, distinct in the first 8 bytes (all `local_replica`
/// reads).
pub fn device(n: u8) -> [u8; 32] {
    let mut id = [n; 32];
    id[..8].copy_from_slice(&u64::from(n).to_be_bytes());
    id
}

/// Run `f` against the document in `store`, returning the delta the commit
/// emits.
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

/// Land `delta` the way the sync path does: decode it into actions and push
/// each through `Interface::apply_action`, skipping the sender's root entry.
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

/// Read `store`'s document through `read`.
pub fn read_with<T: BorshSerialize + BorshDeserialize, R>(
    store: &Store,
    device: [u8; 32],
    read: impl FnOnce(&Root<T>) -> R,
) -> R {
    env::with_runtime_env(env_for(store, device), || {
        read(&Root::<T>::fetch().expect("document root should exist"))
    })
}

/// The `FugueText` document `store` holds.
pub fn fugue_text_in(store: &Store, device: [u8; 32]) -> String {
    read_with::<FugueText<MainStorage>, _>(store, device, |doc| {
        doc.get_text().expect("get_text should succeed")
    })
}

/// A genesis store holding a `FugueText` seeded with `seed`, authored under
/// replica 0 so neither writer's counter space starts used.
pub fn fugue_genesis(field: &str, seed: &str) -> Store {
    let store = new_store();
    clear_pending_delta();
    env::with_runtime_env(env_for(&store, device(1)), || {
        let mut doc = Root::new(|| FugueText::<MainStorage>::new_with_field_name(field));
        doc.insert_str_with_replica(0, 0, seed)
            .expect("seed insert should succeed");
        doc.commit();
        let _ignored = env::take_last_artifact();
    });
    store
}
