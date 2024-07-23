use std::collections::{btree_map, BTreeMap};

use crate::db::Column;
use crate::key::AsKeyParts;
use crate::slice::Slice;

#[derive(Default)]
pub struct Transaction<'a> {
    cols: BTreeMap<Column, BTreeMap<Slice<'a>, Operation<'a>>>,
}

#[derive(Clone)]
pub enum Operation<'a> {
    Put { value: Slice<'a> },
    Delete,
}

impl<'a> Transaction<'a> {
    pub fn get<K: AsKeyParts>(&self, key: &'a K) -> Option<&Operation> {
        self.cols
            .get(&K::column())
            .and_then(|ops| ops.get(key.as_key().as_bytes()))
    }

    pub fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) {
        self.cols
            .entry(K::column())
            .or_default()
            .insert(key.as_key().as_slice(), Operation::Put { value });
    }

    pub fn delete<K: AsKeyParts>(&mut self, key: &'a K) {
        self.cols
            .entry(K::column())
            .or_default()
            .insert(key.as_key().as_slice(), Operation::Delete);
    }

    pub fn merge(&mut self, other: &Transaction<'a>) {
        for (entry, op) in other.iter() {
            self.cols.entry(entry.column).or_default().insert(
                match op {
                    Operation::Put { value } => value.clone(),
                    Operation::Delete => unreachable!(),
                },
                op.clone(),
            );
        }
    }

    pub fn iter(&self) -> Iter<'_, 'a> {
        Iter {
            iter: self.cols.iter(),
            cursor: None,
        }
    }
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

pub struct Iter<'this, 'a> {
    iter: btree_map::Iter<'this, Column, BTreeMap<Slice<'a>, Operation<'a>>>,
    cursor: Option<IterCursor<'this, 'a>>,
}

struct IterCursor<'this, 'a> {
    column: Column,
    iter: btree_map::Iter<'this, Slice<'a>, Operation<'a>>,
}

impl<'this, 'a> Iterator for Iter<'this, 'a> {
    type Item = (Entry<'this>, &'this Operation<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(cursor) = self.cursor.as_mut() {
                if let Some((key, op)) = cursor.iter.next() {
                    return Some((
                        Entry {
                            column: cursor.column,
                            key: key.as_ref(),
                        },
                        op,
                    ));
                }
            }

            let (column, col_iter) = self.iter.next()?;

            self.cursor = Some(IterCursor {
                column: *column,
                iter: col_iter.iter(),
            });
        }
    }
}
