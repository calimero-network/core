use eyre::Result as EyreResult;

use crate::entry::Codec;
use crate::types::PredefinedEntry;
use crate::Store;

/// Collects store writes for documentation and grouping purposes.
/// Currently commits each operation immediately (RocksDB WriteBatch
/// integration is a future enhancement), but provides a clean API
/// for expressing multi-key mutations as a logical unit.
pub struct StoreBatch<'a> {
    store: &'a Store,
    count: usize,
}

impl<'a> StoreBatch<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store, count: 0 }
    }

    pub fn put<K>(&mut self, key: &K, value: &K::DataType<'_>) -> EyreResult<&mut Self>
    where
        K: PredefinedEntry,
        for<'b> <K::Codec as Codec<'b, K::DataType<'b>>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        let mut handle = self.store.handle();
        handle.put(key, value)?;
        self.count += 1;
        Ok(self)
    }

    pub fn delete<K>(&mut self, key: &K) -> EyreResult<&mut Self>
    where
        K: PredefinedEntry,
        for<'b> <K::Codec as Codec<'b, K::DataType<'b>>>::Error:
            std::error::Error + Send + Sync + 'static,
    {
        let mut handle = self.store.handle();
        handle.delete(key)?;
        self.count += 1;
        Ok(self)
    }

    /// Returns the number of operations in this batch.
    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}
