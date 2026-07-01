use eyre::Result as EyreResult;

use crate::entry::Codec;
use crate::slice::Slice;
use crate::tx::Transaction;
use crate::types::PredefinedEntry;
use crate::Store;

/// Accumulates store writes and commits them as a single atomic batch.
///
/// `put`/`delete` stage operations into an in-memory [`Transaction`];
/// nothing reaches the backend until [`commit`](Self::commit), which applies
/// the whole set in one [`Store::apply`] call — a single RocksDB
/// `WriteBatch`, so either every staged op lands or none do. Dropping the
/// batch without committing discards the staged operations.
pub struct StoreBatch<'a> {
    store: &'a Store,
    tx: Transaction<'a>,
    count: usize,
}

impl<'a> StoreBatch<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self {
            store,
            tx: Transaction::default(),
            count: 0,
        }
    }

    /// Stage a `put`. The key and value are encoded into owned bytes up
    /// front, so a serialization error surfaces here — before anything is
    /// committed.
    pub fn put<K>(&mut self, key: &K, value: &K::DataType<'_>) -> EyreResult<&mut Self>
    where
        K: PredefinedEntry,
        for<'b> <K::Codec as Codec<'b, K::DataType<'b>>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        let key_bytes = key.as_key().as_bytes().to_vec();
        let value_bytes = K::Codec::encode(value)?.as_ref().to_vec();
        self.tx.raw_put(
            K::column(),
            Slice::from(key_bytes),
            Slice::from(value_bytes),
        );
        self.count += 1;
        Ok(self)
    }

    /// Stage a `delete`.
    pub fn delete<K>(&mut self, key: &K) -> EyreResult<&mut Self>
    where
        K: PredefinedEntry,
    {
        let key_bytes = key.as_key().as_bytes().to_vec();
        self.tx.raw_delete(K::column(), Slice::from(key_bytes));
        self.count += 1;
        Ok(self)
    }

    /// Returns the number of staged operations.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Commit every staged operation atomically. Consumes the batch; on
    /// error nothing is written (the backend applies the transaction as one
    /// all-or-nothing `WriteBatch`).
    pub fn commit(self) -> EyreResult<()> {
        self.store.apply(&self.tx)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::db::{Column, InMemoryDB};
    use crate::entry::Codec;
    use crate::key::{AsKeyParts, Generic, Key};
    use crate::slice::Slice;
    use crate::types::{GenericData, PredefinedEntry};
    use crate::Store;

    use super::StoreBatch;

    fn store() -> Store {
        Store::new(Arc::new(InMemoryDB::owned()))
    }

    fn gkey(fragment: u8) -> Generic {
        Generic::new([0u8; 16], [fragment; 32])
    }

    fn present(store: &Store, key: &Generic) -> bool {
        store.handle().get(key).expect("read").is_some()
    }

    // Dropping an uncommitted batch writes nothing, even with many staged ops:
    // staging is invisible until `commit`, and abandoning the batch discards it.
    #[test]
    fn drop_persists_nothing() {
        let store = store();
        let (a, b) = (gkey(1), gkey(2));

        let mut dropped = StoreBatch::new(&store);
        dropped
            .put(&a, &GenericData::from(Slice::from(&b"a"[..])))
            .expect("stage a");
        dropped
            .put(&b, &GenericData::from(Slice::from(&b"b"[..])))
            .expect("stage b");
        assert_eq!(dropped.len(), 2);

        // Staged-but-uncommitted keys are not yet observable in the store.
        assert!(
            !present(&store, &a),
            "staged a must be invisible before commit"
        );
        assert!(
            !present(&store, &b),
            "staged b must be invisible before commit"
        );

        drop(dropped);
        assert!(!present(&store, &a), "dropped batch must not persist a");
        assert!(!present(&store, &b), "dropped batch must not persist b");
    }

    // Multi-key all-or-nothing on the happy path: several staged puts stay
    // invisible until `commit`, then all land together. This pins the invariant
    // the one atomic `Store::apply` (a single RocksDB `WriteBatch`) exists to
    // provide.
    #[test]
    fn commit_persists_all_keys() {
        let store = store();
        let (a, b, c) = (gkey(1), gkey(2), gkey(3));

        let mut batch = StoreBatch::new(&store);
        batch
            .put(&a, &GenericData::from(Slice::from(&b"a"[..])))
            .expect("stage a");
        batch
            .put(&b, &GenericData::from(Slice::from(&b"b"[..])))
            .expect("stage b");
        batch
            .put(&c, &GenericData::from(Slice::from(&b"c"[..])))
            .expect("stage c");

        // Nothing is visible while the batch is still staged.
        assert!(
            !present(&store, &a),
            "staged a must be invisible before commit"
        );
        assert!(
            !present(&store, &b),
            "staged b must be invisible before commit"
        );
        assert!(
            !present(&store, &c),
            "staged c must be invisible before commit"
        );

        batch.commit().expect("commit");
        assert!(present(&store, &a) && present(&store, &b) && present(&store, &c));
    }

    // A value that fails to encode surfaces the error from `put` itself — before
    // anything reaches the backend. Because the caller aborts the batch (never
    // reaching `commit`), an earlier good put staged in the same batch is also
    // never written: a mid-batch serialization failure is all-or-nothing too.
    #[test]
    fn encode_failure_surfaces_at_put_before_any_write() {
        // A key whose codec always fails to encode, so `put` must return `Err`.
        // It shares `Generic`'s column and key shape, so a leaked write (if the
        // error did not short-circuit) would be observable via `Handle::get`.
        #[derive(Clone, Copy)]
        struct FailKey(Generic);

        impl AsKeyParts for FailKey {
            type Components = <Generic as AsKeyParts>::Components;

            fn column() -> Column {
                Column::Generic
            }

            fn as_key(&self) -> &Key<Self::Components> {
                self.0.as_key()
            }
        }

        #[derive(Debug)]
        struct EncodeAlwaysFails;

        impl core::fmt::Display for EncodeAlwaysFails {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("intentional encode failure")
            }
        }

        impl std::error::Error for EncodeAlwaysFails {}

        enum FailCodec {}

        impl<'a> Codec<'a, ()> for FailCodec {
            type Error = EncodeAlwaysFails;

            fn encode(_: &()) -> Result<Slice<'_>, Self::Error> {
                Err(EncodeAlwaysFails)
            }

            fn decode(_: Slice<'a>) -> Result<(), Self::Error> {
                Err(EncodeAlwaysFails)
            }
        }

        impl PredefinedEntry for FailKey {
            type Codec = FailCodec;
            type DataType<'a> = ();
        }

        let store = store();
        let good = gkey(1);
        let bad = FailKey(gkey(2));

        let mut batch = StoreBatch::new(&store);
        batch
            .put(&good, &GenericData::from(Slice::from(&b"good"[..])))
            .expect("good put stages");

        // `put` returns `&mut Self` on success, which is not `Debug`, so match
        // rather than `expect_err`.
        let err = match batch.put(&bad, &()) {
            Ok(_) => panic!("encode failure must surface from put"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("intentional encode failure"),
            "unexpected error: {err}"
        );

        // The batch was abandoned mid-way (commit never reached), so not even the
        // earlier good put reached the backend.
        assert!(
            !present(&store, &good),
            "no key may persist when the batch is aborted before commit"
        );
    }
}
