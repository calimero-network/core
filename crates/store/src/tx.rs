use std::collections::btree_map;

use crate::db::Column;
use crate::iter::DBIter;
use crate::key::AsKeyParts;
use crate::slice::Slice;

#[derive(Default)]
pub struct Transaction<'a> {
    ops: btree_map::BTreeMap<Entry<'a>, Operation<'a>>,
}

#[derive(Eq, Ord, Copy, Clone, PartialEq, PartialOrd)]
pub struct Entry<'a> {
    column: Column,
    key: &'a [u8],
}

impl<'a> Entry<'a> {
    pub fn key(&self) -> &'a [u8] {
        self.key
    }

    pub fn column(&self) -> Column {
        self.column
    }
}

impl<'a, T: AsKeyParts> From<&'a T> for Entry<'a> {
    fn from(key: &'a T) -> Self {
        let (column, key) = key.parts();

        Self {
            column,
            key: key.as_bytes(),
        }
    }
}

pub enum Operation<'a> {
    Put { value: Slice<'a> },
    Delete,
}

impl<'a> Transaction<'a> {
    pub fn get(&self, key: &'a impl AsKeyParts) -> Option<&Operation> {
        self.ops.get(&key.into())
    }

    pub fn put(&mut self, key: &'a impl AsKeyParts, value: Slice<'a>) {
        self.ops.insert(key.into(), Operation::Put { value });
    }

    pub fn delete(&mut self, key: &'a impl AsKeyParts) {
        self.ops.insert(key.into(), Operation::Delete);
    }

    pub fn merge(&mut self, other: &Transaction<'a>) {
        for (entry, op) in other.iter() {
            self.ops.insert(
                *entry,
                match op {
                    Operation::Put { value } => Operation::Put {
                        value: value.clone(),
                    },
                    Operation::Delete => Operation::Delete,
                },
            );
        }
    }

    pub fn iter(&self) -> Iter<'_, 'a> {
        Iter {
            inner: self.ops.iter(),
        }
    }

    pub fn iter_range(&self, start: &'a impl AsKeyParts) -> IterRange<'_, 'a> {
        let start = Entry::from(start);

        IterRange {
            col: start.column,
            value: None,
            inner: self.ops.range(start..),
        }
    }
}

pub struct Iter<'this, 'a> {
    inner: btree_map::Iter<'this, Entry<'a>, Operation<'a>>,
}

impl<'this, 'a> Iterator for Iter<'this, 'a> {
    type Item = (&'this Entry<'a>, &'this Operation<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

pub struct IterRange<'this, 'a> {
    col: Column,
    value: Option<&'this Slice<'a>>,
    inner: btree_map::Range<'this, Entry<'a>, Operation<'a>>,
}

impl<'this, 'a> Iterator for IterRange<'this, 'a> {
    type Item = (&'this Entry<'a>, &'this Operation<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a, 'k> DBIter for IterRange<'a, 'k> {
    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        loop {
            let Some((entry, op)) = self.inner.next() else {
                return Ok(None);
            };

            if entry.column() != self.col {
                continue;
            }

            match op {
                Operation::Delete => eyre::bail!("delete operation"),
                Operation::Put { value } => self.value = Some(value),
            };

            return Ok(Some(entry.key().into()));
        }
    }

    fn read(&self) -> Option<Slice> {
        self.value.map(Into::into)
    }
}
