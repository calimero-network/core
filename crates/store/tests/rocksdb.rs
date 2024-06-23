use std::fs;

use calimero_store::config;
use calimero_store::db;
use calimero_store::key::ContextState;
use calimero_store::layer::{temporal, ReadLayer, WriteLayer};
use calimero_store::Store;

#[test]
fn rocks_store() {
    let config = config::StoreConfig {
        path: "corpus/rocks".into(),
    };

    if config.path.exists() {
        if config.path.metadata().unwrap().is_dir() {
            fs::remove_dir_all(&config.path).unwrap();
        } else {
            fs::remove_file(&config.path).unwrap();
        }
    }

    let mut store = Store::open::<db::RocksDB>(&config).unwrap();

    let context_id1 = [0u8; 32];
    let state_key1 = [0u8; 32];
    let key1 = ContextState::new(context_id1, state_key1);

    assert!(!store.has(&key1).unwrap());
    assert_eq!(None, store.get(&key1).unwrap());

    store.put(&key1, b"Hello, World".into()).unwrap();

    assert!(store.has(&key1).unwrap());
    assert_eq!(Some(b"Hello, World".into()), store.get(&key1).unwrap());

    store.put(&key1, b"Some Other Value".into()).unwrap();

    assert!(store.has(&key1).unwrap());
    assert_ne!(Some(b"Hello, World".into()), store.get(&key1).unwrap());
    assert_eq!(Some(b"Some Other Value".into()), store.get(&key1).unwrap());

    let state_key2 = [1u8; 32];
    let key2 = ContextState::new(context_id1, state_key2);

    assert!(store.has(&key1).unwrap());
    assert!(!store.has(&key2).unwrap());

    store.put(&key2, b"Another Value".into()).unwrap();

    assert!(store.has(&key1).unwrap());
    assert!(store.has(&key2).unwrap());

    store.delete(&key1).unwrap();

    assert!(!store.has(&key1).unwrap());
    assert!(store.has(&key2).unwrap());

    store.delete(&key2).unwrap();

    assert!(!store.has(&key1).unwrap());
    assert!(!store.has(&key2).unwrap());

    store.put(&key1, b"Hello, World".into()).unwrap();
    store.put(&key2, b"Another Value".into()).unwrap();

    let mut iter = store.iter(&key1).unwrap();

    let mut keys = iter.keys();

    assert_eq!(Some(key1), keys.next());
    assert_eq!(Some(key2), keys.next());

    let mut iter = store.iter(&key1).unwrap();

    let mut keys = iter.entries();

    assert_eq!(Some((key1, b"Hello, World".into())), keys.next());
    assert_eq!(Some((key2, b"Another Value".into())), keys.next());
}

#[test]
fn temporal_store() {
    let config = config::StoreConfig {
        path: "corpus/temporal".into(),
    };

    if config.path.exists() {
        if config.path.metadata().unwrap().is_dir() {
            fs::remove_dir_all(&config.path).unwrap();
        } else {
            fs::remove_file(&config.path).unwrap();
        }
    }

    let mut store = Store::open::<db::RocksDB>(&config).unwrap();

    let mut store = temporal::Temporal::new(&mut store);

    let context_id1 = [0u8; 32];
    let state_key1 = [0u8; 32];
    let key1 = ContextState::new(context_id1, state_key1);

    assert!(!store.has(&key1).unwrap());
    assert_eq!(None, store.get(&key1).unwrap());

    store.put(&key1, b"Hello, World".into()).unwrap();

    assert!(store.has(&key1).unwrap());
    assert_eq!(Some(b"Hello, World".into()), store.get(&key1).unwrap());

    store.put(&key1, b"Some Other Value".into()).unwrap();

    assert!(store.has(&key1).unwrap());
    assert_ne!(Some(b"Hello, World".into()), store.get(&key1).unwrap());
    assert_eq!(Some(b"Some Other Value".into()), store.get(&key1).unwrap());

    let state_key2 = [1u8; 32];
    let key2 = ContextState::new(context_id1, state_key2);

    assert!(store.has(&key1).unwrap());
    assert!(!store.has(&key2).unwrap());

    store.put(&key2, b"Another Value".into()).unwrap();

    assert!(store.has(&key1).unwrap());
    assert!(store.has(&key2).unwrap());

    store.delete(&key1).unwrap();

    assert!(!store.has(&key1).unwrap());
    assert!(store.has(&key2).unwrap());

    store.delete(&key2).unwrap();

    assert!(!store.has(&key1).unwrap());
    assert!(!store.has(&key2).unwrap());

    store.put(&key1, b"Hello, World".into()).unwrap();
    store.put(&key2, b"Another Value".into()).unwrap();

    let mut iter = store.iter(&key1).unwrap();

    let mut keys = iter.keys();

    assert_eq!(Some(key1), keys.next());
    assert_eq!(Some(key2), keys.next());

    let mut iter = store.iter(&key1).unwrap();

    let mut keys = iter.entries();

    assert_eq!(Some((key1, b"Hello, World".into())), keys.next());
    assert_eq!(Some((key2, b"Another Value".into())), keys.next());
}
