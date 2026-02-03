use core::mem;

use eyre::Ok as EyreOk;

use super::InMemoryDB;
use crate::db::{Column, Database};
use crate::slice::Slice;

#[test]
fn test_owned_memory() {
    let db = InMemoryDB::owned();

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
fn test_ref_memory() {
    let db = InMemoryDB::referenced();

    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];

            let key = Slice::from(&bytes[..]);
            let value = Slice::from(&bytes[..]);

            db.put(
                Column::Identity,
                key.clone().into_boxed().into(),
                value.clone().into_boxed().into(),
            )
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
