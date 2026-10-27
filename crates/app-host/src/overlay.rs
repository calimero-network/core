//! The run's uncommitted writes over the committed state, and lazy range cursors over both.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Bound;

use crate::state::{bounds, KeyBounds, KeyValue, StateView};
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
/// it returned. A cursor holds no iterator between batches: every batch re-ranges the
/// `StateView` and the overlay from the last returned key, so writes made between batches
/// are seen and the cursor (and the `HostCore` holding it) stays `Send + 'static`.
pub(crate) struct Cursor {
    start: Vec<u8>,
    end: Vec<u8>,
    reverse: bool,
    last: Option<Vec<u8>>,
}

impl Cursor {
    /// `start` and `end` are stored keys; an empty `end` is unbounded.
    pub(crate) fn new(start: Vec<u8>, end: Vec<u8>, reverse: bool) -> Self {
        Self {
            start,
            end,
            reverse,
            last: None,
        }
    }

    /// Up to `max` live entries (stored keys). The committed side is read lazily, one
    /// entry ahead of the merge, so at most `max + 1` entries are held at once; `buffered`
    /// is raised to that count.
    pub(crate) fn next_batch(
        &mut self,
        state: &dyn StateView,
        overlay: &Overlay,
        max: usize,
        buffered: &mut usize,
    ) -> Vec<KeyValue> {
        let mut out = Vec::new();
        if max == 0 {
            return out;
        }
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
        let (base_start, base_end) = self.committed_window();
        let mut base = state.range(&base_start, &base_end, self.reverse);
        let mut pending: Option<KeyValue> = None;

        while out.len() < max {
            if pending.is_none() {
                pending = base.next();
            }
            let order = match (&overlay_head, &pending) {
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
            *buffered = (*buffered).max(out.len() + usize::from(pending.is_some()));
            if order == Ordering::Greater {
                // `order` is `Greater` only when `pending` is `Some`.
                let (key, value) = pending.take().expect("pending entry");
                self.last = Some(key.clone());
                out.push((key, value));
            } else {
                if order == Ordering::Equal {
                    // The overlay shadows the committed entry under the same key.
                    pending = None;
                }
                // `order` is `Less` or `Equal` only when `overlay_head` is `Some`.
                let (key, slot) = overlay_head.take().expect("overlay entry");
                overlay_head = overlay_side.next();
                self.last = Some(key.clone());
                if let Some(value) = slot {
                    out.push((key.clone(), value.clone()));
                }
            }
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

    /// The part not yet returned as a `StateView` range: forward from `last ‖ 0x00` (the
    /// smallest key after `last`), reverse up to `last` (exclusive).
    fn committed_window(&self) -> (Vec<u8>, Vec<u8>) {
        let Some(last) = &self.last else {
            return (self.start.clone(), self.end.clone());
        };
        if self.reverse {
            (self.start.clone(), last.clone())
        } else {
            let mut after = last.clone();
            after.push(0);
            (after, self.end.clone())
        }
    }
}
