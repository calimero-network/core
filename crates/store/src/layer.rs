use crate::key::KeyParts;
use crate::slice::Slice;
use crate::tx::Transaction;

pub mod read_only;
pub mod temporal;

pub trait ReadLayer {
    fn has(&self, key: impl KeyParts) -> eyre::Result<bool>;
    fn get(&self, key: impl KeyParts) -> eyre::Result<Option<Slice>>;
}

pub trait WriteLayer: ReadLayer {
    type Base: WriteLayer;

    fn put(&mut self, key: impl KeyParts, value: Slice) -> eyre::Result<()>;
    fn delete(&mut self, key: impl KeyParts) -> eyre::Result<()>;
    fn apply(&mut self, tx: Transaction) -> eyre::Result<()>;

    fn commit(self) -> eyre::Result<Self::Base>;
}

impl ReadLayer for crate::Store {
    fn has(&self, key: impl KeyParts) -> eyre::Result<bool> {
        let col = key.column();
        let key = key.key().as_slice();

        self.db.has(col, key)
    }

    fn get(&self, key: impl KeyParts) -> eyre::Result<Option<Slice>> {
        let col = key.column();
        let key = key.key().as_slice();

        self.db.get(col, key)
    }
}

impl WriteLayer for crate::Store {
    type Base = Self;

    fn put(&mut self, key: impl KeyParts, value: Slice) -> eyre::Result<()> {
        let col = key.column();
        let key = key.key().as_slice();

        self.db.put(col, key, value)
    }

    fn delete(&mut self, key: impl KeyParts) -> eyre::Result<()> {
        let col = key.column();
        let key = key.key().as_slice();

        self.db.delete(col, key)
    }

    fn apply(&mut self, tx: Transaction) -> eyre::Result<()> {
        self.db.apply(tx)
    }

    fn commit(self) -> eyre::Result<Self::Base> {
        Ok(self)
    }
}
