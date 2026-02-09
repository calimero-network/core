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
}
