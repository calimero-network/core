use core::mem;

use eyre::Ok as EyreOk;

use super::InMemoryDB;
use crate::db::{Column, Database};
use crate::slice::Slice;

/// The default `delete_range` (used by every non-RocksDB backend) must delete
/// the whole `[lo, hi)` slice while keeping peak memory bounded — it processes
/// in batches and re-seeks. Insert more than one batch so the loop runs
/// multiple times, then confirm the range is gone and the flanks survive.
#[test]
fn delete_range_default_deletes_full_range_across_batches() {
    let db = InMemoryDB::owned();

    // Big-endian keys sort in numeric order, so `[lo, hi)` on the bytes matches
    // the numeric interval. `N` exceeds one internal batch (4096) so the
    // re-seek loop is exercised more than once.
    const N: u32 = 5_000;
    for i in 0..N {
        let key = i.to_be_bytes();
        db.put(Column::Identity, (&key[..]).into(), (&key[..]).into())
            .expect("put should succeed");
    }

    let lo = 1_000_u32.to_be_bytes();
    let hi = 4_000_u32.to_be_bytes();
    db.delete_range(Column::Identity, (&lo[..]).into(), (&hi[..]).into())
        .expect("delete_range should succeed");

    for i in 0..N {
        let key = i.to_be_bytes();
        let present = db
            .has(Column::Identity, (&key[..]).into())
            .expect("has should succeed");
        let expected = !(1_000..4_000).contains(&i);
        assert_eq!(present, expected, "key {i} presence after delete_range");
    }
}

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
