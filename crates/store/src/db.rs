pub mod rocksdb;

pub type Key = Vec<u8>;
pub type Value = Vec<u8>;

pub trait Database: Send + Sync {
    fn get(&self, key: &Key) -> eyre::Result<Option<Value>>;
    fn put(&self, key: &Key, value: Value) -> eyre::Result<()>;
    fn apply(&self, tx: Transaction) -> eyre::Result<()>;
}

#[derive(Default)]
pub struct Transaction {
    ops: Vec<Operation>,
}

pub enum Operation {
    Put { key: Key, value: Value },
    Delete { key: Key },
}

impl Transaction {
    pub fn put(&mut self, key: Key, value: Value) {
        self.ops.push(Operation::Put { key, value });
    }

    pub fn delete(&mut self, key: Key) {
        self.ops.push(Operation::Delete { key });
    }
}
