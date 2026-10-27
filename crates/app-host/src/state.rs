//! Read access to committed app state (D46) and the generation-prefixed key layout (D39).

use std::collections::BTreeMap;
use std::ops::Bound;

/// A key and its value.
pub type KeyValue = (Vec<u8>, Vec<u8>);

/// Lower and upper bounds of a key range.
pub(crate) type KeyBounds<'k> = (Bound<&'k [u8]>, Bound<&'k [u8]>);

/// The generation every app uses in this milestone; rebuilds allocate new ones (D38, D39).
pub const GENERATION: u64 = 0;

/// The stored key of an app key: 8-byte big-endian generation ‖ key.
#[must_use]
pub fn app_key(generation: u64, key: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + key.len());
    out.extend_from_slice(&generation.to_be_bytes());
    out.extend_from_slice(key);
    out
}

/// App state at one applied index. Keys are full stored keys, generation included.
pub trait StateView: Send + Sync {
    /// The value stored under `key`.
    fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

    /// Entries in `[start, end)` in key order, or in reverse order when `reverse` is set.
    /// An empty `end` is unbounded. Implementations yield entries lazily.
    fn range<'s>(
        &'s self,
        start: &[u8],
        end: &[u8],
        reverse: bool,
    ) -> Box<dyn Iterator<Item = KeyValue> + 's>;
}

impl StateView for BTreeMap<Vec<u8>, Vec<u8>> {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        BTreeMap::get(self, key).cloned()
    }

    fn range<'s>(
        &'s self,
        start: &[u8],
        end: &[u8],
        reverse: bool,
    ) -> Box<dyn Iterator<Item = KeyValue> + 's> {
        let Some(bounds) = bounds(start, end) else {
            return Box::new(std::iter::empty());
        };
        let iter = BTreeMap::range::<[u8], _>(self, bounds).map(|(k, v)| (k.clone(), v.clone()));
        if reverse {
            Box::new(iter.rev())
        } else {
            Box::new(iter)
        }
    }
}

/// `[start, end)` as range bounds, with an empty `end` unbounded; `None` when empty,
/// so `BTreeMap::range` never sees `start > end`.
pub(crate) fn bounds<'k>(start: &'k [u8], end: &'k [u8]) -> Option<KeyBounds<'k>> {
    if end.is_empty() {
        return Some((Bound::Included(start), Bound::Unbounded));
    }
    if start >= end {
        return None;
    }
    Some((Bound::Included(start), Bound::Excluded(end)))
}
