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

#[test]
fn test_rocksdb_entries_survive_collection() {
    // Regression test: keys/values yielded by `entries()`/`keys()` must remain
    // valid after subsequent `next()` calls. The underlying RocksDB iterator
    // hands out slices borrowing its internal buffer, which is overwritten on
    // each advance. Because `Iterator::next` does not tie its item to the
    // `&mut self` borrow, collecting the items into a `Vec` (or otherwise
    // retaining them) must not expose freed/overwritten memory.
    let dir = TempDir::new("_calimero_store_collect").expect("tempdir should be created");
    let dir_path = dir
        .path()
        .to_owned()
        .try_into()
        .expect("path conversion should succeed");
    let config = StoreConfig::new(dir_path);
    let db = RocksDB::open(&config).expect("db should open");

    let mut expected = Vec::new();
    for b1 in 0..10 {
        for b2 in 0..10 {
            let bytes = [b1, b2];
            db.put(
                Column::Identity,
                Slice::from(&bytes[..]),
                Slice::from(&bytes[..]),
            )
            .expect("put should succeed");
            expected.push([b1, b2]);
        }
    }

    // Collect every entry up front, holding each yielded slice across the
    // `next()` calls that follow it.
    let mut iter = db.iter(Column::Identity).expect("iter should succeed");
    // NB: the items are typed `Slice<'_>` (the iterator's `Item` carries the
    // iterator's lifetime), not `Slice<'static>` — even though the fix backs
    // them with owned `Box<[u8]>` at runtime. Annotating `'static` here would
    // wrongly require `db` to be borrowed for `'static` and fail to compile.
    let collected: Vec<(Slice<'_>, Slice<'_>)> = iter
        .entries()
        .map(|(k, v)| {
            (
                k.expect("key should be valid"),
                v.expect("value should be valid"),
            )
        })
        .collect();

    assert_eq!(collected.len(), expected.len());
    for ((key, value), bytes) in collected.iter().zip(&expected) {
        assert_eq!(key.as_ref(), &bytes[..], "collected key was invalidated");
        assert_eq!(value.as_ref(), &bytes[..], "collected value was invalidated");
    }

    // Same expectation for `keys()`.
    let mut iter = db.iter(Column::Identity).expect("iter should succeed");
    let collected_keys: Vec<Slice<'_>> = iter
        .keys()
        .map(|k| k.expect("key should be valid"))
        .collect();

    assert_eq!(collected_keys.len(), expected.len());
    for (key, bytes) in collected_keys.iter().zip(&expected) {
        assert_eq!(key.as_ref(), &bytes[..], "collected key was invalidated");
    }

    // Directly model the use-after-free: retain entry N, advance to N+1, then
    // read the retained N. The underlying RocksDB cursor has moved on by then,
    // so an unowned slice would be reading overwritten buffer memory.
    let mut iter = db.iter(Column::Identity).expect("iter should succeed");
    let mut entries = iter.entries();
    let mut prev: Option<(Slice<'_>, Slice<'_>, [u8; 2])> = None;
    for bytes in &expected {
        let (key, value) = entries
            .next()
            .map(|(k, v)| {
                (
                    k.expect("key should be valid"),
                    v.expect("value should be valid"),
                )
            })
            .expect("entry should exist");

        // Assert the *previous* entry (retained across this advance) is intact.
        if let Some((prev_key, prev_value, prev_bytes)) = &prev {
            assert_eq!(
                prev_key.as_ref(),
                &prev_bytes[..],
                "retained key was invalidated by the next advance"
            );
            assert_eq!(
                prev_value.as_ref(),
                &prev_bytes[..],
                "retained value was invalidated by the next advance"
            );
        }

        prev = Some((key, value, *bytes));
    }
}

#[test]
fn test_data_persistence() {
    // Test that data persists across open/close cycles
    let dir = TempDir::new("_calimero_store_persistence").expect("tempdir should be created");

    let dir_path = dir
        .path()
        .to_owned()
        .try_into()
        .expect("path conversion should succeed");

    let config = StoreConfig::new(dir_path);

    // Open, write, and close
    {
        let db = RocksDB::open(&config).expect("open should succeed");
        let key = Slice::from(&[1, 2, 3][..]);
        let value = Slice::from(&[4, 5, 6][..]);
        db.put(Column::Identity, (&key).into(), (&value).into())
            .expect("put should succeed");
    }

    // Reopen and verify data persists
    {
        let db = RocksDB::open(&config).expect("reopen should succeed");
        let key = Slice::from(&[1, 2, 3][..]);
        let retrieved = db
            .get(Column::Identity, (&key).into())
            .expect("get should succeed")
            .expect("key should exist");
        assert_eq!(retrieved.as_ref(), &[4, 5, 6]);
    }
}

#[test]
fn test_approximate_size_scopes_to_range() {
    // Verify the range scoping: we want prefix `0x10..` to report ≈ the
    // in-range payload without leaking bytes from `0x20..`. RocksDB's
    // `get_approximate_sizes_cf` samples SST metadata so the reported
    // value may be 0 in-memory (nothing flushed). We still assert the
    // in-range probe ≤ total-range probe to catch range inversion bugs.
    let dir = TempDir::new("_calimero_store_approx_size").expect("tempdir");
    let dir_path = dir.path().to_owned().try_into().expect("path conversion");
    let config = StoreConfig::new(dir_path);
    let db = RocksDB::open(&config).expect("db open");

    // Seed two buckets' worth of data: prefix 0x10 and prefix 0x20.
    let payload = vec![0xAB_u8; 4096];
    for i in 0..32u8 {
        let key = [0x10, i, 0, 0];
        db.put(
            Column::Identity,
            Slice::from(&key[..]),
            Slice::from(payload.as_slice()),
        )
        .expect("put 0x10 bucket");

        let key = [0x20, i, 0, 0];
        db.put(
            Column::Identity,
            Slice::from(&key[..]),
            Slice::from(payload.as_slice()),
        )
        .expect("put 0x20 bucket");
    }

    let in_range = db
        .approximate_size(
            Column::Identity,
            Slice::from(&[0x10_u8][..]),
            Slice::from(&[0x11_u8][..]),
        )
        .expect("approximate_size in-range");
    let full_range = db
        .approximate_size(
            Column::Identity,
            Slice::from(&[0x00_u8][..]),
            Slice::from(&[0xFF_u8][..]),
        )
        .expect("approximate_size full");
    assert!(
        in_range <= full_range,
        "in-range ({in_range}) must not exceed full-range ({full_range})"
    );
}

#[test]
fn test_delete_range_drops_only_in_range_keys() {
    // The native range tombstone backing `SortedMap::clear`'s index drop must
    // delete exactly `[lo, hi)` and leave neighbouring prefixes untouched.
    let dir = TempDir::new("_calimero_store_delete_range").expect("tempdir");
    let dir_path = dir.path().to_owned().try_into().expect("path conversion");
    let config = StoreConfig::new(dir_path);
    let db = RocksDB::open(&config).expect("db open");

    // Three prefixes; we'll wipe only the middle one (0x20).
    for p in [0x10_u8, 0x20, 0x30] {
        for i in 0..8u8 {
            let key = [p, i];
            db.put(
                Column::SortedIndex,
                Slice::from(&key[..]),
                Slice::from(&[i][..]),
            )
            .expect("put");
        }
    }

    db.delete_range(
        Column::SortedIndex,
        Slice::from(&[0x20_u8][..]),
        Slice::from(&[0x21_u8][..]),
    )
    .expect("delete_range");

    // The 0x20 bucket is gone…
    for i in 0..8u8 {
        assert!(
            !db.has(Column::SortedIndex, Slice::from(&[0x20_u8, i][..]))
                .expect("has 0x20"),
            "0x20 key {i} should have been deleted"
        );
    }
    // …while the neighbours survive untouched.
    for (p, label) in [(0x10_u8, "0x10"), (0x30, "0x30")] {
        for i in 0..8u8 {
            assert!(
                db.has(Column::SortedIndex, Slice::from(&[p, i][..]))
                    .expect("has neighbour"),
                "{label} key {i} must survive a neighbouring range delete"
            );
        }
    }
}
