use std::fmt::Debug;

use strum::{AsRefStr, EnumIter};

use crate::config::StoreConfig;
use crate::iter::Iter;
use crate::slice::Slice;
use crate::tx::Transaction;

mod memory;
mod rocksdb;

pub use memory::InMemoryDB;
pub use rocksdb::RocksDB;

#[derive(AsRefStr, Clone, Copy, Debug, EnumIter, Eq, Ord, PartialEq, PartialOrd)]
#[non_exhaustive]
pub enum Column {
    Meta,
    Identity,
    State,
    Transaction,
    Blobs,
    Application,
    Generic,
}

pub trait Database<'a>: Debug + Send + Sync + 'static {
    fn open(config: &StoreConfig) -> eyre::Result<Self>
    where
        Self: Sized;

    fn has(&self, col: Column, key: Slice<'_>) -> eyre::Result<bool>;
    fn get(&self, col: Column, key: Slice<'_>) -> eyre::Result<Option<Slice<'_>>>;
    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> eyre::Result<()>;
    fn delete(&self, col: Column, key: Slice<'_>) -> eyre::Result<()>;

    // TODO: We should consider returning Iterator here.
    #[allow(clippy::iter_not_returning_iterator)]
    fn iter(&self, col: Column) -> eyre::Result<Iter<'_>>;

    // todo! redesign this, each DB should return a transaction
    // todo! modelled similar to Iter - {put, delete, clear}
    fn apply(&self, tx: &Transaction<'a>) -> eyre::Result<()>;
}
