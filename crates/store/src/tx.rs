use std::collections::btree_map::{self, BTreeMap};
use std::mem::MaybeUninit;

use crate::db::Column;
use crate::key::KeyParts;

type Key = Box<[u8]>;
type Value = Box<[u8]>;

#[derive(Default)]
pub struct Transaction {
    ops: BTreeMap<Entry, Operation>,
}

pub struct Entry {
    pub column: Column,
    pub key: Key,
    _ref: MaybeUninit<(u8, (&'static Column, &'static [u8]))>,
}

impl Entry {
    fn new(column: Column, key: Key) -> Self {
        Self {
            column,
            key,
            _ref: MaybeUninit::uninit(),
        }
    }
}

impl Ord for Entry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match self.column.cmp(&other.column) {
            std::cmp::Ordering::Equal => {}
            ord => return ord,
        }
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.column.partial_cmp(&other.column) {
            Some(core::cmp::Ordering::Equal) => {}
            ord => return ord,
        }
        self.key.partial_cmp(&other.key)
    }
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.column == other.column && self.key == other.key
    }
}

impl Eq for Entry {}

impl<'a> std::borrow::Borrow<(&'a Column, &'a [u8])> for Entry {
    fn borrow(&self) -> &(&'a Column, &'a [u8]) {
        let (tag, data) = unsafe { &mut *self._ref.as_ptr().cast_mut() };

        if *tag == 0 {
            *tag = 1;
            data.0 = &self.column;
            data.1 = &*self.key;
        };

        unsafe { std::mem::transmute(data) }
    }
}

pub enum Operation {
    Put { value: Value },
    Delete,
}

impl Transaction {
    pub fn get(&self, key: impl KeyParts) -> Option<&Operation> {
        let column = key.column();
        let key = key.key().as_bytes();

        self.ops.get(&(&column, key))
    }

    pub fn put(&mut self, key: impl KeyParts, value: Value) {
        let column = key.column();
        let key = key.key().as_bytes();

        self.ops
            .insert(Entry::new(column, key.into()), Operation::Put { value });
    }

    pub fn delete(&mut self, key: impl KeyParts) {
        let column = key.column();
        let key = key.key().as_bytes();

        self.ops
            .insert(Entry::new(column, key.into()), Operation::Delete);
    }

    pub fn merge(&mut self, other: Transaction) {
        self.ops.extend(other.ops);
    }
}

impl IntoIterator for Transaction {
    type Item = (Entry, Operation);
    type IntoIter = btree_map::IntoIter<Entry, Operation>;

    fn into_iter(self) -> Self::IntoIter {
        self.ops.into_iter()
    }
}
