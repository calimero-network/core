use core::mem;

use calimero_store::config::StoreConfig;
use calimero_store::db::{Column, Database};
use calimero_store::slice::Slice;
use eyre::Ok as EyreOk;
use tempdir::TempDir;

use crate::RocksDB;

#[test]
fn test_rocksdb() {
    let dir = TempDir::new("_calimero_store_rocksdb").expect("tempdir should be created");

    let dir_path = dir
        .path()
        .to_owned()
        .try_into()
        .expect("path conversion should succeed");

    let config = StoreConfig::new(dir_path);

    let db = RocksDB::open(&config).expect("db should open");

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let key = Slice::from(&bytes[..]);
            let value = Slice::from(&bytes[..]);

            db.put(Column::Identity, (&key).into(), (&value).into())
                .expect("put should succeed");

            assert!(db
                .has(Column::Identity, (&key).into())
                .expect("has should succeed"));
            assert_eq!(
                db.get(Column::Identity, key)
                    .expect("get should succeed")
                    .expect("key should exist"),
                value
            );
        }
    }

    assert_eq!(
        None,
        db.get(Column::Identity, (&[]).into())
            .expect("get should succeed")
    );

    let mut iter = db.iter(Column::Identity).expect("iter should succeed");

    let mut key = Some(
        iter.seek((&[]).into())
            .expect("seek should succeed")
            .expect("seek should find a key")
            .into_boxed(),
    );
    let mut value = Some(
        iter.read()
            .expect("read should succeed")
            .clone()
            .into_boxed(),
    );

    let mut entries = iter.entries();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let (k, v) = entries
                .next()
                .map(|(k, v)| EyreOk((k?, v?)))
                .transpose()
                .expect("entry iteration should succeed")
                .map_or_else(Default::default, |(k, v)| {
                    (Some(k.into_boxed()), Some(v.into_boxed()))
                });

            let last_key = mem::replace(&mut key, k).expect("key should be present");
            let last_value = mem::replace(&mut value, v).expect("value should be present");

            let bytes = [b1, b2];

            assert_eq!(bytes, &*last_key);
            assert_eq!(bytes, &*last_value);
        }
    }
}

#[test]
fn test_rocksdb_iter() {
    let dir = TempDir::new("_calimero_store_rocks").expect("tempdir should be created");

    let dir_path = dir
        .path()
        .to_owned()
        .try_into()
        .expect("path conversion should succeed");

    let config = StoreConfig::new(dir_path);

    let db = RocksDB::open(&config).expect("db should open");

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let key = Slice::from(&bytes[..]);
            let value = Slice::from(&bytes[..]);

            db.put(Column::Identity, (&key).into(), (&value).into())
                .expect("put should succeed");

            assert!(db
                .has(Column::Identity, (&key).into())
                .expect("has should succeed"));
            assert_eq!(
                db.get(Column::Identity, key)
                    .expect("get should succeed")
                    .expect("key should exist"),
                value
            );
        }
    }

    let mut iter = db.iter(Column::Identity).expect("iter should succeed");

    let mut entries = iter.entries();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let (key, value) = entries
                .next()
                .map(|(k, v)| {
                    (
                        k.expect("key should be valid"),
                        v.expect("value should be valid"),
                    )
                })
                .expect("entry should exist");

            assert_eq!(key, bytes);
            assert_eq!(value, bytes);
        }
    }
}
