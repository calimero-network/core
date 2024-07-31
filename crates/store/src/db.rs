use strum::{AsRefStr, EnumIter};

use crate::config::StoreConfig;
use crate::iter::Iter;
use crate::slice::Slice;
use crate::tx::Transaction;

mod memory;
mod rocksdb;

pub use memory::InMemoryDB;
pub use rocksdb::RocksDB;

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, EnumIter, AsRefStr)]
pub enum Column {
    Meta,
    Identity,
    State,
    Transaction,
    Application,
    Generic,
}

pub trait Database<'a>: Send + Sync + 'static {
    fn open(config: &StoreConfig) -> eyre::Result<Self>
    where
        Self: Sized;

    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool>;
    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>>;
    fn put(&self, col: Column, key: Slice<'a>, value: Slice<'a>) -> eyre::Result<()>;
    fn delete(&self, col: Column, key: Slice) -> eyre::Result<()>;
    fn iter(&self, col: Column) -> eyre::Result<Iter>;

    // todo! redesign this, each DB should return a transaction
    // todo! modelled similar to Iter - {put, delete, clear}
    fn apply(&self, tx: &Transaction<'a>) -> eyre::Result<()>;
}
