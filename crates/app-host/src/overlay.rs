//! The run's uncommitted writes over the committed state, and lazy range cursors over both.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Bound;

use crate::state::{bounds, KeyBounds, KeyValue};
use crate::types::Write;

/// Ordered writes of one run, keyed by stored key; `None` is a tombstone.
#[derive(Debug, Default)]
pub(crate) struct Overlay {
    slots: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}

impl Overlay {
    /// `Some(None)` when the run deleted the key, `None` when it never touched it.
    pub(crate) fn get(&self, key: &[u8]) -> Option<Option<&[u8]>> {
        self.slots.get(key).map(Option::as_deref)
    }

    pub(crate) fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        let _ = self.slots.insert(key, Some(value));
    }

    pub(crate) fn delete(&mut self, key: Vec<u8>) {
        let _ = self.slots.insert(key, None);
    }

    /// The writes in key order.
    pub(crate) fn into_diff(self) -> Vec<Write> {
        self.slots
            .into_iter()
            .map(|(key, value)| Write { key, value })
            .collect()
    }
}

/// A range over the overlay merged with the committed state, resumed from the last key
/// it returned. The committed side is one lazy iterator kept across batches (the state
/// does not change during a run); the overlay side is re-read from the last key on every
/// batch, so writes made between batches are seen.
pub(crate) struct Cursor<'a> {
    start: Vec<u8>,
    end: Vec<u8>,
    reverse: bool,
    last: Option<Vec<u8>>,
    base: Box<dyn Iterator<Item = KeyValue> + 'a>,
    /// One committed entry read ahead of the merge and not yet returned.
    pending: Option<KeyValue>,
}

impl<'a> Cursor<'a> {
    /// `start` and `end` are stored keys; an empty `end` is unbounded.
    pub(crate) fn new(
        start: Vec<u8>,
        end: Vec<u8>,
        reverse: bool,
        base: Box<dyn Iterator<Item = KeyValue> + 'a>,
    ) -> Self {
        Self {
            start,
            end,
            reverse,
            last: None,
            base,
            pending: None,
        }
    }

    /// Up to `max` live entries (stored keys). `buffered` is raised to the most entries
    /// this cursor held at once: the batch plus the read-ahead entry.
    pub(crate) fn next_batch(
        &mut self,
        overlay: &Overlay,
        max: usize,
        buffered: &mut usize,
    ) -> Vec<KeyValue> {
        let mut out = Vec::new();
        let Some(window) = self.remaining() else {
            return out;
        };
        let overlay_side = overlay.slots.range::<[u8], _>(window);
        let mut overlay_side: Box<dyn Iterator<Item = _>> = if self.reverse {
            Box::new(overlay_side.rev())
        } else {
            Box::new(overlay_side)
        };
        let mut overlay_head = overlay_side.next();

        while out.len() < max {
            if self.pending.is_none() {
                self.pending = self.base.next();
            }
            let order = match (&overlay_head, &self.pending) {
                (None, None) => break,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some((ok, _)), Some((bk, _))) => {
                    let order = ok.as_slice().cmp(bk.as_slice());
                    if self.reverse {
                        order.reverse()
                    } else {
                        order
                    }
                }
            };
            if order == Ordering::Greater {
                // SAFETY: `order` is `Greater` only when `pending` is `Some`.
                let (key, value) = self.pending.take().expect("pending entry");
                self.last = Some(key.clone());
                out.push((key, value));
            } else {
                if order == Ordering::Equal {
                    // The overlay shadows the committed entry under the same key.
                    self.pending = None;
                }
                // SAFETY: `order` is `Less` or `Equal` only when `overlay_head` is `Some`.
                let (key, slot) = overlay_head.take().expect("overlay entry");
                overlay_head = overlay_side.next();
                self.last = Some(key.clone());
                if let Some(value) = slot {
                    out.push((key.clone(), value.clone()));
                }
            }
            *buffered = (*buffered).max(out.len() + usize::from(self.pending.is_some()));
        }
        out
    }

    /// The part of `[start, end)` not yet returned, as overlay bounds.
    fn remaining(&self) -> Option<KeyBounds<'_>> {
        let (lower, upper) = bounds(&self.start, &self.end)?;
        let Some(last) = self.last.as_deref() else {
            return Some((lower, upper));
        };
        if self.reverse {
            Some((lower, Bound::Excluded(last)))
        } else {
            Some((Bound::Excluded(last), upper))
        }
    }
}
