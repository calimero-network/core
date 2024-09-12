use core::fmt::Debug;

use eyre::Result as EyreResult;
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
    Config,
    Identity,
    State,
    Transaction,
    Blobs,
    Application,
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
    #[allow(clippy::iter_not_returning_iterator)]
    fn iter(&self, col: Column) -> EyreResult<Iter<'_>>;

    // todo! redesign this, each DB should return a transaction
    // todo! modelled similar to Iter - {put, delete, clear}
    fn apply(&self, tx: &Transaction<'a>) -> EyreResult<()>;
}
