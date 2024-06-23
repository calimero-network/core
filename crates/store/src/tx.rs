use std::collections::btree_map;

use crate::db::Column;
use crate::iter::DBIter;
use crate::key::AsKeyParts;
use crate::slice::Slice;

#[derive(Default)]
pub struct Transaction<'k, 'v> {
    ops: btree_map::BTreeMap<Entry<'k>, Operation<'v>>,
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

impl<'k, 'v> Transaction<'k, 'v> {
    pub fn get(&self, key: &'k impl AsKeyParts) -> Option<&Operation> {
        self.ops.get(&key.into())
    }

    pub fn put(&mut self, key: &'k impl AsKeyParts, value: Slice<'v>) {
        self.ops.insert(key.into(), Operation::Put { value });
    }

    pub fn delete(&mut self, key: &'k impl AsKeyParts) {
        self.ops.insert(key.into(), Operation::Delete);
    }

    pub fn merge(&mut self, other: &Transaction<'k, 'v>) {
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

    pub fn iter(&self) -> Iter<'_, 'k, 'v> {
        Iter {
            inner: self.ops.iter(),
        }
    }

    pub fn iter_range(&self, start: &'k impl AsKeyParts) -> IterRange<'_, 'k, 'v> {
        let start = Entry::from(start);

        IterRange {
            col: start.column,
            value: None,
            inner: self.ops.range(start..),
        }
    }
}

pub struct Iter<'a, 'k, 'v> {
    inner: btree_map::Iter<'a, Entry<'k>, Operation<'v>>,
}

impl<'a, 'k, 'v> Iterator for Iter<'a, 'k, 'v> {
    type Item = (&'a Entry<'k>, &'a Operation<'v>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

pub struct IterRange<'a, 'k, 'v> {
    col: Column,
    value: Option<&'a Slice<'v>>,
    inner: btree_map::Range<'a, Entry<'k>, Operation<'v>>,
}

impl<'a, 'k, 'v> Iterator for IterRange<'a, 'k, 'v> {
    type Item = (&'a Entry<'k>, &'a Operation<'v>);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

impl<'a, 'k, 'v> DBIter for IterRange<'a, 'k, 'v> {
    fn next(&mut self) -> eyre::Result<Option<Slice>> {
        let Some((entry, op)) = self.inner.next() else {
            return Ok(None);
        };

        assert_ne!(entry.column(), self.col, "column mismatch");

        match op {
            Operation::Delete => eyre::bail!("delete operation"),
            Operation::Put { value } => {
                self.value = Some(value);

                return Ok(Some(entry.key().into()));
            }
        }
    }

    fn read(&self) -> Option<Slice> {
        self.value.map(Into::into)
    }
}
