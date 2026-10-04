use core::mem;
use std::time::{Duration, Instant};

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

fn key(i: u32) -> [u8; 4] {
    i.to_be_bytes()
}

fn put_n(db: &InMemoryDB<super::Owned>, n: u32) {
    for i in 0..n {
        db.put(Column::Identity, (&key(i)[..]).into(), (&key(i)[..]).into())
            .expect("put should succeed");
    }
}

/// Every key the iterator yields from the start, with its value.
fn drain(iter: &mut crate::iter::Iter<'_>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let mut next = iter
        .next()
        .expect("next should succeed")
        .map(|k| k.to_vec());
    while let Some(k) = next {
        let v = iter.read().expect("read should succeed").to_vec();
        out.push((k, v));
        next = iter
            .next()
            .expect("next should succeed")
            .map(|k| k.to_vec());
    }
    out
}

/// An iterator reads the column as it was when the iterator was made: a write,
/// overwrite or delete made while it is alive is invisible to it, and visible to
/// the next one.
#[test]
fn an_iterator_reads_a_snapshot_whatever_is_written_during_it() {
    let db = InMemoryDB::owned();
    put_n(&db, 4);

    let mut during = db.iter(Column::Identity).expect("iter");
    db.put(Column::Identity, (&key(9)[..]).into(), (&key(9)[..]).into())
        .expect("put");
    db.put(Column::Identity, (&key(1)[..]).into(), (&b"new"[..]).into())
        .expect("overwrite");
    db.delete(Column::Identity, (&key(2)[..]).into())
        .expect("delete");

    let before: Vec<_> = (0..4).map(|i| (key(i).to_vec(), key(i).to_vec())).collect();
    assert_eq!(drain(&mut during), before);
    drop(during);

    let mut after = db.iter(Column::Identity).expect("iter");
    assert_eq!(
        drain(&mut after),
        vec![
            (key(0).to_vec(), key(0).to_vec()),
            (key(1).to_vec(), b"new".to_vec()),
            (key(3).to_vec(), key(3).to_vec()),
            (key(9).to_vec(), key(9).to_vec()),
        ]
    );
}

/// A value overwritten or deleted while an iterator holds it stays readable
/// through that iterator, and its arena slot is reclaimed when the last
/// snapshot holding it goes — not before, and not never.
#[test]
fn the_last_snapshot_to_go_reclaims_what_only_it_held() {
    let db = InMemoryDB::owned();
    put_n(&db, 4);
    let len = || db.inner.inner.read().expect("lock").arena_len();
    assert_eq!(len(), 4);

    let first = db.iter(Column::Identity).expect("iter");
    let second = db.iter(Column::Identity).expect("iter");
    db.put(Column::Identity, (&key(1)[..]).into(), (&b"new"[..]).into())
        .expect("overwrite");
    db.delete(Column::Identity, (&key(2)[..]).into())
        .expect("delete");
    // The old value of 1 and the deleted 2 are held by the snapshots.
    assert_eq!(len(), 5);

    drop(first);
    assert_eq!(len(), 5, "the other snapshot still holds them");
    drop(second);
    assert_eq!(len(), 3, "the last snapshot reclaims them");

    // With no iterator alive, overwrites and deletes reclaim at once.
    db.put(Column::Identity, (&key(0)[..]).into(), (&b"x"[..]).into())
        .expect("overwrite");
    db.delete(Column::Identity, (&key(3)[..]).into())
        .expect("delete");
    assert_eq!(len(), 2);
}

/// Making an iterator and reading a few keys from it must not cost more as the
/// column grows. It used to copy the whole column per iterator, which made every
/// prefix scan in a test O(rows in the column). Timed, fastest of three, at a
/// small and a 32x larger column: a copy grows ~32x, a snapshot ~1x.
#[test]
fn an_iterator_does_not_cost_more_as_the_column_grows() {
    const SCANS: usize = 200;
    let measure = |rows: u32| -> Duration {
        let db = InMemoryDB::owned();
        put_n(&db, rows);
        (0..3)
            .map(|_| {
                let start = Instant::now();
                for _ in 0..SCANS {
                    let mut iter = db.iter(Column::Identity).expect("iter");
                    let first = iter
                        .seek((&key(rows / 2)[..]).into())
                        .expect("seek")
                        .map(|k| k.to_vec());
                    assert_eq!(first, Some(key(rows / 2).to_vec()));
                    let _next = iter.next().expect("next");
                }
                start.elapsed()
            })
            .min()
            .expect("three runs")
    };

    let _warm = measure(1_000);
    let small = measure(1_000);
    let large = measure(32_000);
    let growth = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        growth <= 4.0,
        "1000 -> 32000 rows took {small:?} -> {large:?} for {SCANS} scans, {growth:.1}x; \
         a snapshot is ~1x, a copy of the column ~32x"
    );
}
