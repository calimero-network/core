use core::fmt::Debug;

use eyre::Result as EyreResult;
use strum::{AsRefStr, EnumIter};

use crate::config::StoreConfig;
use crate::iter::Iter;
use crate::slice::Slice;
use crate::tx::Transaction;

mod memory;

pub use memory::InMemoryDB;

#[derive(AsRefStr, Clone, Copy, Debug, EnumIter, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Column {
    Meta,
    Config,
    Identity,
    State,
    /// Node-local private storage that is NOT synchronized across nodes
    PrivateState,
    Delta,
    Blobs,
    Application,
    Alias,
    Generic,
    Group,
}

pub trait Database<'a>: Debug + Send + Sync + 'static {
    fn open(config: &StoreConfig) -> EyreResult<Self>
    where
        Self: Sized;

    fn has(&self, col: Column, key: Slice<'_>) -> EyreResult<bool>;
    fn get(&self, col: Column, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>>;
    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> EyreResult<()>;
    fn delete(&self, col: Column, key: Slice<'_>) -> EyreResult<()>;

    // TODO: We should consider returning Iterator here.
    #[expect(
        clippy::iter_not_returning_iterator,
        reason = "TODO: This should be implemented"
    )]
    fn iter(&self, col: Column) -> EyreResult<Iter<'_>>;

    // todo! redesign this, each DB should return a transaction
    // todo! modelled similar to Iter - {put, delete, clear}
    fn apply(&self, tx: &Transaction<'a>) -> EyreResult<()>;

    /// Returns an iterator over a column with a consistent snapshot view.
    ///
    /// The iterator sees a frozen point-in-time view of the database,
    /// unaffected by concurrent writes. This is essential for operations
    /// that need to iterate over a consistent state (e.g., snapshot generation).
    ///
    /// The default implementation falls back to `iter()` for databases that
    /// don't support snapshots natively.
    fn iter_snapshot(&self, col: Column) -> EyreResult<Iter<'_>> {
        self.iter(col)
    }

    /// Best-effort estimate of on-disk bytes stored in `col` for keys in
    /// `[start, end)`. Used by usage-reporting endpoints to measure
    /// per-group / per-context storage cheaply (RocksDB returns a real
    /// approximation from SST metadata, no scan). Callers must not
    /// depend on exact byte counts.
    ///
    /// Default implementation walks the column (seeking to `start`) and
    /// sums actual `key.len() + value.len()` for matching entries.
    /// Backends backed by sorted storage should override with something
    /// cheaper (RocksDB: `get_approximate_sizes_cf`).
    fn approximate_size(
        &self,
        col: Column,
        start: Slice<'_>,
        end: Slice<'_>,
    ) -> EyreResult<u64> {
        let mut iter = self.iter(col)?;
        let mut total: u64 = 0;
        let end_bytes: Vec<u8> = end.as_ref().to_vec();

        // Seek to `start` and drive the iteration to sum matching entries.
        // We copy each key's bytes out (to release the mutable borrow),
        // `read()` the value (immutable borrow), record the size, then
        // drop both and advance.
        let mut current: Option<Vec<u8>> = iter.seek(start)?.map(|k| k.as_ref().to_vec());
        while let Some(key_bytes) = current {
            if key_bytes.as_slice() >= end_bytes.as_slice() {
                break;
            }
            let value_len = {
                let value = iter.read()?;
                value.as_ref().len() as u64
            };
            total = total.saturating_add(key_bytes.len() as u64);
            total = total.saturating_add(value_len);
            current = iter.next()?.map(|k| k.as_ref().to_vec());
        }
        Ok(total)
    }
}
