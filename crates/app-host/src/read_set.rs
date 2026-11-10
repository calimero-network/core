//! What a view read (PoC, view subscriptions): every key it got and every range it scanned,
//! as stored keys (generation prefix included, as in a diff), so the node can tell whether an
//! applied entry's diff changed anything the view's answer depends on.

use std::collections::BTreeSet;

use crate::types::Write;

/// Keys one read set holds at most; a view that reads more cannot be watched.
pub const MAX_READ_SET_KEYS: usize = 4_096;
/// Ranges (after merging overlapping and adjacent ones) one read set holds at most.
pub const MAX_READ_SET_RANGES: usize = 64;
/// Bytes of keys and range bounds one read set holds at most (what a node keeps per watch).
pub const MAX_READ_SET_BYTES: usize = 256 * 1024;

/// The keys and ranges one view run read. A set over [`MAX_READ_SET_KEYS`],
/// [`MAX_READ_SET_RANGES`] or [`MAX_READ_SET_BYTES`], or a run that read something a set of keys cannot describe (another
/// app's view), is [`ReadSet::unbounded`]: it stops recording and touches every write, and the
/// node refuses to watch it rather than watch a cut-down set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadSet {
    keys: BTreeSet<Vec<u8>>,
    /// `[start, end)`, sorted by start, never overlapping or adjacent; an empty `end` is the
    /// end of the keyspace.
    ranges: Vec<(Vec<u8>, Vec<u8>)>,
    /// Bytes of `keys`.
    key_bytes: usize,
    /// Bytes of `ranges`' bounds.
    range_bytes: usize,
    unbounded: Option<String>,
}

impl ReadSet {
    /// Why the set cannot stand for what the view read, if it cannot.
    #[must_use]
    pub fn unbounded(&self) -> Option<&str> {
        self.unbounded.as_deref()
    }

    /// The keys got, as stored keys.
    #[must_use]
    pub fn keys(&self) -> &BTreeSet<Vec<u8>> {
        &self.keys
    }

    /// The ranges scanned, `[start, end)` as stored keys (an empty `end`: unbounded).
    #[must_use]
    pub fn ranges(&self) -> &[(Vec<u8>, Vec<u8>)] {
        &self.ranges
    }

    /// Whether any write of `diff` lands on a key or in a range of the set; an unbounded
    /// set is touched by everything.
    #[must_use]
    pub fn touches(&self, diff: &[Write]) -> bool {
        diff.iter().any(|w| self.touches_key(&w.key))
    }

    /// Whether a write to the stored key `key` lands in the set.
    #[must_use]
    pub fn touches_key(&self, key: &[u8]) -> bool {
        if self.unbounded.is_some() || self.keys.contains(key) {
            return true;
        }
        // The last range starting at or before `key` is the only one that can hold it.
        let at = self
            .ranges
            .partition_point(|(start, _)| start.as_slice() <= key);
        at > 0 && before_end(key, &self.ranges[at - 1].1)
    }

    /// Records a key got.
    pub(crate) fn key(&mut self, key: &[u8]) {
        if self.unbounded.is_some() || self.keys.contains(key) {
            return;
        }
        if self.keys.len() >= MAX_READ_SET_KEYS {
            self.give_up(format!("the view read more than {MAX_READ_SET_KEYS} keys"));
            return;
        }
        self.key_bytes = self.key_bytes.saturating_add(key.len());
        let _new = self.keys.insert(key.to_vec());
        self.check_bytes();
    }

    /// Bytes of keys and range bounds the set holds.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.key_bytes.saturating_add(self.range_bytes)
    }

    fn check_bytes(&mut self) {
        if self.bytes() > MAX_READ_SET_BYTES {
            self.give_up(format!(
                "the view's reads take more than {MAX_READ_SET_BYTES} bytes to describe"
            ));
        }
    }

    /// Records `[start, end)` scanned (an empty `end`: to the end of the keyspace), merged
    /// with the ranges it overlaps or adjoins.
    pub(crate) fn range(&mut self, start: Vec<u8>, end: Vec<u8>) {
        if self.unbounded.is_some() || (!end.is_empty() && end <= start) {
            return;
        }
        let (mut start, mut end) = (start, end);
        let mut kept = Vec::with_capacity(self.ranges.len() + 1);
        for (s, e) in self.ranges.drain(..) {
            // Disjoint and not adjacent: `e < start` or `end < s`.
            let left = !e.is_empty() && e < start;
            let right = !end.is_empty() && end < s;
            if left || right {
                kept.push((s, e));
                continue;
            }
            if s < start {
                start = s;
            }
            if e.is_empty() || (!end.is_empty() && e > end) {
                end = e;
            }
        }
        let at = kept.partition_point(|(s, _)| *s < start);
        kept.insert(at, (start, end));
        self.ranges = kept;
        if self.ranges.len() > MAX_READ_SET_RANGES {
            self.give_up(format!(
                "the view scanned more than {MAX_READ_SET_RANGES} ranges"
            ));
            return;
        }
        self.range_bytes = self.ranges.iter().map(|(s, e)| s.len() + e.len()).sum();
        self.check_bytes();
    }

    /// Marks the set as unable to stand for the run's reads.
    pub(crate) fn give_up(&mut self, reason: String) {
        if self.unbounded.is_none() {
            self.unbounded = Some(reason);
        }
        self.keys.clear();
        self.ranges.clear();
        self.key_bytes = 0;
        self.range_bytes = 0;
    }
}

/// `key < end`, an empty `end` being past every key.
fn before_end(key: &[u8], end: &[u8]) -> bool {
    end.is_empty() || key < end
}

/// One open cursor's reads so far: what it will add to the read set when it closes.
#[derive(Clone, Debug)]
pub(crate) struct CursorRead {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
    pub reverse: bool,
    /// The last key a batch returned (stored form).
    pub last: Option<Vec<u8>>,
    /// The cursor answered fewer entries than asked: it read to its far end.
    pub exhausted: bool,
}

impl CursorRead {
    /// Adds what the cursor read to `set`: the whole range once it was exhausted, else the
    /// part up to and including the last key it returned (a write beyond that key cannot
    /// change what the view saw); nothing if it returned nothing and stopped early.
    pub fn close(self, set: &mut ReadSet) {
        if self.exhausted {
            set.range(self.start, self.end);
            return;
        }
        let Some(last) = self.last else {
            return;
        };
        if self.reverse {
            set.range(last, self.end);
        } else {
            let mut after = last;
            after.push(0);
            set.range(self.start, after);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(key: &[u8]) -> Write {
        Write {
            key: key.to_vec(),
            value: Some(b"v".to_vec()),
        }
    }

    #[test]
    fn ranges_merge_and_bound_their_writes() {
        let mut set = ReadSet::default();
        set.range(b"b".to_vec(), b"d".to_vec());
        set.range(b"f".to_vec(), b"h".to_vec());
        set.range(b"d".to_vec(), b"e".to_vec());
        assert_eq!(
            set.ranges(),
            &[
                (b"b".to_vec(), b"e".to_vec()),
                (b"f".to_vec(), b"h".to_vec())
            ]
        );
        assert!(set.touches(&[w(b"b")]));
        assert!(set.touches(&[w(b"dz")]));
        assert!(!set.touches(&[w(b"e")]));
        assert!(!set.touches(&[w(b"a")]));
        assert!(!set.touches(&[w(b"h")]));
        set.range(b"a".to_vec(), Vec::new());
        assert_eq!(set.ranges(), &[(b"a".to_vec(), Vec::new())]);
        assert!(set.touches(&[w(b"zzz")]));
    }

    #[test]
    fn a_set_over_its_caps_is_unbounded_not_cut() {
        let mut set = ReadSet::default();
        for i in 0..=MAX_READ_SET_KEYS {
            set.key(&(i as u32).to_be_bytes());
        }
        assert!(set.unbounded().is_some());
        assert!(set.touches(&[w(b"anything")]));

        let mut set = ReadSet::default();
        for i in 0..=MAX_READ_SET_RANGES as u32 {
            let start = (i * 4).to_be_bytes().to_vec();
            let end = (i * 4 + 1).to_be_bytes().to_vec();
            set.range(start, end);
        }
        assert!(set.unbounded().is_some());
    }
}
