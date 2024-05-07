use strum::{AsRefStr, EnumIter};

use crate::slice::Slice;
use crate::tx::Transaction;

pub mod rocksdb;

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, EnumIter, AsRefStr)]
pub enum Column {
    Identity,
    State,
    Transaction,
    Membership,
}

pub trait Database {
    fn has(&self, col: Column, key: Slice) -> eyre::Result<bool>;
    fn get(&self, col: Column, key: Slice) -> eyre::Result<Option<Slice>>;
    fn put(&self, col: Column, key: Slice, value: Slice) -> eyre::Result<()>;
    fn delete(&self, col: Column, key: Slice) -> eyre::Result<()>;

    fn apply(&self, tx: Transaction) -> eyre::Result<()>;
}
