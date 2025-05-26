#![allow(unused_crate_dependencies, reason = "False positives")]

// use std::fs::{remove_dir_all, remove_file};

// use calimero_store::key::{ContextIdentity as ContextIdentityKey, ContextState as ContextStateKey};
// use calimero_store::layer::{ReadLayer, WriteLayer};
// use calimero_store::{config, db, Store};
// use eyre::Ok as EyreOk;

// #[test]
// fn rocks_store() {
//     let config = config::StoreConfig::new("corpus/rocks".into());

//     if config.path.exists() {
//         if config.path.metadata().unwrap().is_dir() {
//             remove_dir_all(&config.path).unwrap();
//         } else {
//             remove_file(&config.path).unwrap();
//         }
//     }

//     let mut store = Store::open::<db::RocksDB>(&config).unwrap();

//     let context_id1 = [0_u8; 32].into();
//     let state_key1 = [0_u8; 32];
//     let key1 = ContextStateKey::new(context_id1, state_key1);

//     assert!(!store.has(&key1).unwrap());
//     assert_eq!(None, store.get(&key1).unwrap());

//     store.put(&key1, b"Hello, World".into()).unwrap();

//     assert!(store.has(&key1).unwrap());
//     assert_eq!(Some(b"Hello, World".into()), store.get(&key1).unwrap());

//     store.put(&key1, b"Some Other Value".into()).unwrap();

//     assert!(store.has(&key1).unwrap());
//     assert_ne!(Some(b"Hello, World".into()), store.get(&key1).unwrap());
//     assert_eq!(Some(b"Some Other Value".into()), store.get(&key1).unwrap());

//     let state_key2 = [1_u8; 32];
//     let key2 = ContextStateKey::new(context_id1, state_key2);

//     assert!(store.has(&key1).unwrap());
//     assert!(!store.has(&key2).unwrap());

//     store.put(&key2, b"Another Value".into()).unwrap();

//     assert!(store.has(&key1).unwrap());
//     assert!(store.has(&key2).unwrap());

//     store.delete(&key1).unwrap();

//     assert!(!store.has(&key1).unwrap());
//     assert!(store.has(&key2).unwrap());

//     store.delete(&key2).unwrap();

//     assert!(!store.has(&key1).unwrap());
//     assert!(!store.has(&key2).unwrap());

//     store.put(&key1, b"Hello, World".into()).unwrap();
//     store.put(&key2, b"Another Value".into()).unwrap();

//     {
//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.keys();

//         assert_eq!(Some(key1), keys.next().transpose().unwrap());
//         assert_eq!(Some(key2), keys.next().transpose().unwrap());
//         assert_eq!(None, keys.next().transpose().unwrap());

//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.entries();

//         assert_eq!(
//             Some((key1, b"Hello, World".into())),
//             keys.next()
//                 .map(|(k, v)| EyreOk((k?, v?)))
//                 .transpose()
//                 .unwrap()
//         );
//         assert_eq!(
//             Some((key2, b"Another Value".into())),
//             keys.next()
//                 .map(|(k, v)| EyreOk((k?, v?)))
//                 .transpose()
//                 .unwrap()
//         );
//         assert_eq!(
//             None,
//             keys.next()
//                 .map(|(k, v)| EyreOk((k?, v?)))
//                 .transpose()
//                 .unwrap()
//         );
//     }

//     let public_key1 = [0_u8; 32];

//     let key3 = ContextIdentityKey::new(context_id1, public_key1.into());

//     store.put(&key3, b"Some Associated Value".into()).unwrap();

//     let public_key2 = [1_u8; 32];

//     let key4 = ContextIdentityKey::new(context_id1, public_key2.into());

//     store
//         .put(&key4, b"Another Associated Value".into())
//         .unwrap();

//     {
//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.keys();

//         assert_eq!(Some(key1), keys.next().transpose().unwrap());
//         assert_eq!(Some(key2), keys.next().transpose().unwrap());
//         assert_eq!(None, keys.next().transpose().unwrap());
//     }

//     {
//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.keys();

//         assert_eq!(Some(key3), keys.next().transpose().unwrap());
//         assert_eq!(Some(key4), keys.next().transpose().unwrap());
//         assert_eq!(None, keys.next().transpose().unwrap());
//     }
// }

// #[test]
// fn temporal_store() {
//     let config = config::StoreConfig {
//         path: "corpus/temporal".into(),
//     };

//     if config.path.exists() {
//         if config.path.metadata().unwrap().is_dir() {
//             remove_dir_all(&config.path).unwrap();
//         } else {
//             remove_file(&config.path).unwrap();
//         }
//     }

//     let mut store = Store::open::<db::RocksDB>(&config).unwrap();

//     let mut store = Temporal::new(&mut store);

//     let context_id1 = [0u8; 32].into();
//     let state_key1 = [0u8; 32];
//     let key1 = ContextStateKey::new(context_id1, state_key1);

//     assert!(!store.has(&key1).unwrap());
//     assert_eq!(None, store.get(&key1).unwrap());

//     store.put(&key1, b"Hello, World".into()).unwrap();

//     assert!(store.has(&key1).unwrap());
//     assert_eq!(Some(b"Hello, World".into()), store.get(&key1).unwrap());

//     store.put(&key1, b"Some Other Value".into()).unwrap();

//     assert!(store.has(&key1).unwrap());
//     assert_ne!(Some(b"Hello, World".into()), store.get(&key1).unwrap());
//     assert_eq!(Some(b"Some Other Value".into()), store.get(&key1).unwrap());

//     let state_key2 = [1u8; 32];
//     let key2 = ContextStateKey::new(context_id1, state_key2);

//     assert!(store.has(&key1).unwrap());
//     assert!(!store.has(&key2).unwrap());

//     store.put(&key2, b"Another Value".into()).unwrap();

//     assert!(store.has(&key1).unwrap());
//     assert!(store.has(&key2).unwrap());

//     store.delete(&key1).unwrap();

//     assert!(!store.has(&key1).unwrap());
//     assert!(store.has(&key2).unwrap());

//     store.delete(&key2).unwrap();

//     assert!(!store.has(&key1).unwrap());
//     assert!(!store.has(&key2).unwrap());

//     store.put(&key1, b"Hello, World".into()).unwrap();
//     store.put(&key2, b"Another Value".into()).unwrap();

//     {
//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.keys();

//         assert_eq!(Some(key1), keys.next().transpose().unwrap());
//         assert_eq!(Some(key2), keys.next().transpose().unwrap());
//         assert_eq!(None, keys.next().transpose().unwrap());

//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.entries();

//         assert_eq!(
//             Some((key1, b"Hello, World".into())),
//             keys.next()
//                 .map(|(k, v)| EyreOk((k?, v?)))
//                 .transpose()
//                 .unwrap()
//         );
//         assert_eq!(
//             Some((key2, b"Another Value".into())),
//             keys.next()
//                 .map(|(k, v)| EyreOk((k?, v?)))
//                 .transpose()
//                 .unwrap()
//         );
//         assert_eq!(
//             None,
//             keys.next()
//                 .map(|(k, v)| EyreOk((k?, v?)))
//                 .transpose()
//                 .unwrap()
//         );
//     }

//     let public_key1 = [0u8; 32];

//     let key3 = ContextIdentityKey::new(context_id1, public_key1.into());

//     store.put(&key3, b"Some Associated Value".into()).unwrap();

//     let public_key2 = [1u8; 32];

//     let key4 = ContextIdentityKey::new(context_id1, public_key2.into());

//     store
//         .put(&key4, b"Another Associated Value".into())
//         .unwrap();

//     {
//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.keys();

//         assert_eq!(Some(key1), keys.next().transpose().unwrap());
//         assert_eq!(Some(key2), keys.next().transpose().unwrap());
//         assert_eq!(None, keys.next().transpose().unwrap());
//     }

//     {
//         let mut iter = store.iter().unwrap();

//         let mut keys = iter.keys();

//         assert_eq!(Some(key3), keys.next().transpose().unwrap());
//         assert_eq!(Some(key4), keys.next().transpose().unwrap());
//         assert_eq!(None, keys.next().transpose().unwrap());
//     }
// }
