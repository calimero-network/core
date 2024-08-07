use strum::{AsRefStr, EnumIter};

use crate::config::StoreConfig;
use crate::iter::Iter;
use crate::slice::Slice;
use crate::tx::Transaction;

// todo!
// mod memory;
mod rocksdb;

pub use rocksdb::RocksDB;

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, EnumIter, AsRefStr)]
pub enum Column {
    Identity,
    State,
    Transaction,
    Application,
    Generic,
}

pub trait Database: Send + Sync + 'static {
    fn open(config: &StoreConfig) -> eyre::Result<Self>
    where
        Self: Sized;

    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool>;
    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>>;
    fn put(&self, col: Column, key: Slice, value: Slice) -> eyre::Result<()>;
    fn delete(&self, col: Column, key: Slice) -> eyre::Result<()>;
    fn iter(&self, col: Column, key: Slice) -> eyre::Result<Iter>;

    fn apply(&self, tx: &Transaction) -> eyre::Result<()>;
}
