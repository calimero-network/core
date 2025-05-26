use core::ops::Bound;
use std::collections::btree_map::{Iter as BTreeIter, Range};
use std::collections::BTreeMap;

use crate::db::Column;
use crate::key::AsKeyParts;
use crate::slice::Slice;

#[derive(Default, Debug)]
pub struct Transaction<'a> {
    cols: BTreeMap<Column, BTreeMap<Slice<'a>, Operation<'a>>>,
}

#[derive(Clone, Debug)]
pub enum Operation<'a> {
    Put { value: Slice<'a> },
    Delete,
}

impl<'a> Transaction<'a> {
    pub fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    pub(crate) fn raw_get(&self, column: Column, key: &[u8]) -> Option<&Operation<'_>> {
        self.cols.get(&column).and_then(|ops| ops.get(key))
    }

    pub fn get<K: AsKeyParts>(&self, key: &K) -> Option<&Operation<'_>> {
        self.cols.get(&K::column())?.get(key.as_key().as_bytes())
    }

    pub fn put<K: AsKeyParts>(&mut self, key: &'a K, value: Slice<'a>) {
        drop(
            self.cols
                .entry(K::column())
                .or_default()
                .insert(key.as_key().as_slice(), Operation::Put { value }),
        );
    }

    pub fn delete<K: AsKeyParts>(&mut self, key: &'a K) {
        drop(
            self.cols
                .entry(K::column())
                .or_default()
                .insert(key.as_key().as_slice(), Operation::Delete),
        );
    }

    #[expect(clippy::use_self, reason = "Needed in order to specify a lifetime")]
    pub fn merge(&mut self, other: &Transaction<'a>) {
        for (entry, op) in other.iter() {
            drop(self.cols.entry(entry.column).or_default().insert(
                match op {
                    Operation::Put { value } => value.clone(),
                    Operation::Delete => unreachable!(),
                },
                op.clone(),
            ));
        }
    }

    pub fn iter(&self) -> Iter<'_, 'a> {
        Iter {
            iter: self.cols.iter(),
            cursor: None,
        }
    }

    pub(crate) fn col_iter(&self, col: Column, start: Option<&[u8]>) -> ColRange<'_, 'a> {
        ColRange {
            iter: self.cols.get(&col).map(|col| {
                col.range::<[u8], _>((
                    start.map_or_else(|| Bound::Unbounded, Bound::Included),
                    Bound::Unbounded,
                ))
            }),
        }
    }
}

#[derive(Debug)]
pub struct ColRange<'this, 'a> {
    iter: Option<Range<'this, Slice<'a>, Operation<'a>>>,
}

impl<'this, 'a> Iterator for ColRange<'this, 'a> {
    type Item = (Slice<'this>, &'this Operation<'a>);

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.as_mut()?.next().map(|(k, v)| (k.into(), v))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Entry<'a> {
    column: Column,
    key: &'a [u8],
}

impl<'a> Entry<'a> {
    pub const fn key(&self) -> &'a [u8] {
        self.key
    }

    pub const fn column(&self) -> Column {
        self.column
    }
}

#[derive(Debug)]
pub struct Iter<'this, 'a> {
    iter: BTreeIter<'this, Column, BTreeMap<Slice<'a>, Operation<'a>>>,
    cursor: Option<IterCursor<'this, 'a>>,
}

#[derive(Debug)]
struct IterCursor<'this, 'a> {
    column: Column,
    iter: BTreeIter<'this, Slice<'a>, Operation<'a>>,
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
