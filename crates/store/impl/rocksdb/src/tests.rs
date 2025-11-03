use core::mem;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::slice::Slice;
use eyre::Ok as EyreOk;
use tempdir::TempDir;

use crate::RocksDB;

#[test]
fn test_rocksdb() {
    let dir = TempDir::new("_calimero_store_rocksdb").unwrap();

    let dir_path = dir.path().to_owned().try_into().unwrap();

    let config = StoreConfig::new(dir_path);

    let db = RocksDB::open(&config).unwrap();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let key = Slice::from(&bytes[..]);
            let value = Slice::from(&bytes[..]);

            db.put(Column::Identity, (&key).into(), (&value).into())
                .unwrap();

            assert!(db.has(Column::Identity, (&key).into()).unwrap());
            assert_eq!(db.get(Column::Identity, key).unwrap().unwrap(), value);
        }
    }

    assert_eq!(None, db.get(Column::Identity, (&[]).into()).unwrap());

    let mut iter = db.iter(Column::Identity).unwrap();

    let mut key = Some(iter.seek((&[]).into()).unwrap().unwrap().into_boxed());
    let mut value = Some(iter.read().unwrap().clone().into_boxed());

    let mut entries = iter.entries();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let (k, v) = entries
                .next()
                .map(|(k, v)| EyreOk((k?, v?)))
                .transpose()
                .unwrap()
                .map_or_else(Default::default, |(k, v)| {
                    (Some(k.into_boxed()), Some(v.into_boxed()))
                });

            let last_key = mem::replace(&mut key, k).unwrap();
            let last_value = mem::replace(&mut value, v).unwrap();

            let bytes = [b1, b2];

            assert_eq!(bytes, &*last_key);
            assert_eq!(bytes, &*last_value);
        }
    }
}

#[test]
fn test_rocksdb_iter() {
    let dir = TempDir::new("_calimero_store_rocks").unwrap();

    let dir_path = dir.path().to_owned().try_into().unwrap();

    let config = StoreConfig::new(dir_path);

    let db = RocksDB::open(&config).unwrap();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let key = Slice::from(&bytes[..]);
            let value = Slice::from(&bytes[..]);

            db.put(Column::Identity, (&key).into(), (&value).into())
                .unwrap();

            assert!(db.has(Column::Identity, (&key).into()).unwrap());
            assert_eq!(db.get(Column::Identity, key).unwrap().unwrap(), value);
        }
    }

    let mut iter = db.iter(Column::Identity).unwrap();

    let mut entries = iter.entries();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let (key, value) = entries
                .next()
                .map(|(k, v)| (k.unwrap(), v.unwrap()))
                .unwrap();

            assert_eq!(key, bytes);
            assert_eq!(value, bytes);
        }
    }
}
