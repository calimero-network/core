//! Recorded bytes for a map entry's stored layout.
//!
//! The layout is not private: `decode_rotation_log_entry_child` reads it
//! positionally straight out of RocksDB, and app-defined merge dispatch needs
//! the value at a known offset. A silent reordering would break both, so the
//! exact bytes are pinned here rather than left to whatever the tuple happens
//! to be.
//!
//! Own test binary — `ROOT_ID` is a process-wide `LazyLock` derived from
//! `context_id()`, so a sibling test that touches a collection outside a
//! `RuntimeEnv` freezes it at the no-env fallback and genesis here fails.

#![cfg(feature = "testing")]
#![allow(clippy::unwrap_used)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::address::Id;
use calimero_storage::collections::crdt_meta::MergeError;
use calimero_storage::collections::rekey::field_child_id;
use calimero_storage::collections::{MergeStrategy, Mergeable, Root, UnorderedMap};
use calimero_storage::env::{self, RuntimeEnv};
use calimero_storage::store::Key;
use calimero_storage::{register_crdt_merge_for_test, rekey_field_if_supported};
use serial_test::serial;

type Store = Rc<RefCell<HashMap<[u8; 32], Vec<u8>>>>;

fn env_for(s: &Store) -> RuntimeEnv {
    let r = s.clone();
    let reader = Rc::new(move |k: &Key| r.borrow().get(&k.to_bytes()).cloned());
    let w = s.clone();
    let writer =
        Rc::new(move |k: Key, v: &[u8]| w.borrow_mut().insert(k.to_bytes(), v.to_vec()).is_some());
    let rm = s.clone();
    let remover = Rc::new(move |k: &Key| rm.borrow_mut().remove(&k.to_bytes()).is_some());
    RuntimeEnv::new(reader, writer, remover, [7u8; 32], [1u8; 32], [0xACu8; 32])
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct App {
    items: UnorderedMap<String, Counted>,
}

/// A value whose borsh encoding is short and unmistakable in a hex dump.
#[derive(BorshSerialize, BorshDeserialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct Counted {
    n: u32,
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for Counted {
    const DISPATCHED: bool = false;
}

impl Mergeable for Counted {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.n = self.n.max(other.n);
        Ok(())
    }
}

impl calimero_storage::collections::rekey::RekeyTarget for Counted {
    fn rekey_relative_to(&mut self, _parent_id: Id) {}
}

// Structural: a test fixture, merged by the storage layer's own rules.
#[diagnostic::do_not_recommend]
impl MergeStrategy for App {
    const DISPATCHED: bool = false;
}

impl Mergeable for App {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.items.merge(&other.items)
    }
}

impl calimero_storage::collections::rekey::RekeyTarget for App {
    fn rekey_relative_to(&mut self, parent_id: Id) {
        rekey_field_if_supported!(&mut self.items, field_child_id(parent_id, "items"));
    }
}

/// One entry, one known key, one known value — then read the raw stored bytes.
///
/// `AABBCCDD` little-endian is `0xDDCCBBAA`, chosen so the value is impossible
/// to confuse with the key's length prefix or with the trailing id.
#[test]
#[serial]
fn a_map_entry_stores_the_value_before_the_key() {
    env::reset_environment();
    register_crdt_merge_for_test::<App>();

    let store: Store = Default::default();
    env::with_runtime_env(env_for(&store), || {
        Root::new(App::default).commit();
    });

    let entry_bytes = env::with_runtime_env(env_for(&store), || {
        let mut app = Root::<App>::fetch().unwrap();
        app.items
            .insert("ab".to_owned(), Counted { n: 0xDDCC_BBAA })
            .unwrap();
        app.commit();

        // Find the one child whose bytes contain the value marker.
        let marker = [0xAA, 0xBB, 0xCC, 0xDD];
        store
            .borrow()
            .values()
            .find(|v| v.windows(4).any(|w| w == marker))
            .cloned()
            .expect("the entry must be in the store")
    });

    // value (4) ++ key len (4) ++ key (2) ++ element id (32) = 42
    assert_eq!(
        entry_bytes.len(),
        42,
        "unexpected entry size: {entry_bytes:02x?}"
    );

    assert_eq!(
        &entry_bytes[..4],
        &[0xAA, 0xBB, 0xCC, 0xDD],
        "the VALUE must come first — merge dispatch decodes it at offset 0 \
         without knowing the key's type. Got: {entry_bytes:02x?}"
    );
    assert_eq!(
        &entry_bytes[4..10],
        &[2, 0, 0, 0, b'a', b'b'],
        "the key follows the value, borsh-framed"
    );
}
