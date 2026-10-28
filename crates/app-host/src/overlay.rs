//! The run's uncommitted writes over the committed state, and lazy range cursors over both.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ops::Bound;

use crate::state::{bounds, KeyBounds, KeyValue, StateView};
use crate::types::Write;

/// One write to a key: the run's write sequence number and the value (`None` deletes).
type Version = (u64, Option<Vec<u8>>);

/// Ordered writes of one run, keyed by stored key. Every write is stamped with the run's
/// next sequence number and kept, oldest first, so a cursor can read the writes as they
/// were when it was opened. Versions are bounded by `MAX_WRITES` per run.
#[derive(Debug, Default)]
pub(crate) struct Overlay {
    slots: BTreeMap<Vec<u8>, Vec<Version>>,
    seq: u64,
}

impl Overlay {
    /// The sequence number of the latest write; a cursor opened now sees writes up to it.
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }

    /// The newest write: `Some(None)` when the run deleted the key, `None` when it never
    /// touched it.
    pub(crate) fn get(&self, key: &[u8]) -> Option<Option<&[u8]>> {
        self.slots
            .get(key)
            .and_then(|versions| versions.last())
            .map(|(_, value)| value.as_deref())
    }

    pub(crate) fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.write(key, Some(value));
    }

    pub(crate) fn delete(&mut self, key: Vec<u8>) {
        self.write(key, None);
    }

    fn write(&mut self, key: Vec<u8>, value: Option<Vec<u8>>) {
        self.seq += 1;
        self.slots.entry(key).or_default().push((self.seq, value));
    }

    /// The newest write per key, in key order.
    pub(crate) fn into_diff(self) -> Vec<Write> {
        self.slots
            .into_iter()
            .filter_map(|(key, mut versions)| versions.pop().map(|(_, value)| Write { key, value }))
            .collect()
    }
}

/// The newest version written at or before `seq`, if any.
fn as_of(versions: &[Version], seq: u64) -> Option<&Option<Vec<u8>>> {
    versions
        .iter()
        .rev()
        .find(|(written, _)| *written <= seq)
        .map(|(_, value)| value)
}

/// A range over the overlay merged with the committed state, resumed from the last key
/// it returned. It reads the state as of its opening: the committed state (fixed for the
/// run) plus the writes made before `range_open`, never later ones, so a scan that moves
/// records ahead of itself does not meet them again. It holds no iterator between batches:
/// every batch re-ranges the `StateView` and the overlay from the last returned key, so
/// the cursor (and the `HostCore` holding it) stays `Send + 'static`.
pub(crate) struct Cursor {
    start: Vec<u8>,
    end: Vec<u8>,
    reverse: bool,
    /// The overlay sequence at opening; later writes are invisible to this cursor.
    seq: u64,
    last: Option<Vec<u8>>,
}

impl Cursor {
    /// `start` and `end` are stored keys; an empty `end` is unbounded.
    pub(crate) fn new(start: Vec<u8>, end: Vec<u8>, reverse: bool, seq: u64) -> Self {
        Self {
            start,
            end,
            reverse,
            seq,
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
        let seq = self.seq;
        let overlay_side = overlay.slots.range::<[u8], _>(window);
        let overlay_side: Box<dyn Iterator<Item = _>> = if self.reverse {
            Box::new(overlay_side.rev())
        } else {
            Box::new(overlay_side)
        };
        // Keys with no write before opening fall through to the committed state.
        let mut overlay_side = overlay_side
            .filter_map(|(key, versions)| as_of(versions, seq).map(|value| (key, value)));
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
                // SAFETY: guaranteed by the match above: `order` is `Greater` only when `pending` is `Some`.
                let (key, value) = pending.take().expect("pending entry");
                self.last = Some(key.clone());
                out.push((key, value));
            } else {
                if order == Ordering::Equal {
                    // The overlay shadows the committed entry under the same key.
                    pending = None;
                }
                // SAFETY: guaranteed by the match above: `order` is `Less` or `Equal` only when `overlay_head` is `Some`.
                let (key, slot) = overlay_head.take().expect("overlay entry");
                overlay_head = overlay_side.next();
                self.last = Some(key.clone());
                if let Some(value) = slot {
                    out.push((key.clone(), value.clone()));
                }
            }
        }
        *buffered = (*buffered).max(out.len() + usize::from(pending.is_some()));
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
