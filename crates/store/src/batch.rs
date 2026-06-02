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
