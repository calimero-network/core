use core::cmp::Ordering;

use eyre::Result as EyreResult;

use crate::iter::{DBIter, Iter, Structured};
use crate::key::{AsKeyParts, FromKeyParts};
use crate::layer::{Layer, ReadLayer, WriteLayer};
use crate::slice::Slice;
use crate::tx::{self, Operation, Transaction};

#[derive(Debug)]
pub struct Temporal<'base, 'entry, L> {
    inner: &'base mut L,
    shadow: Transaction<'entry>,
}

impl<'base, 'entry, L> Temporal<'base, 'entry, L>
where
    L: WriteLayer<'entry>,
{
    pub fn new(layer: &'base mut L) -> Self {
        Self {
            inner: layer,
            shadow: Transaction::default(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shadow.is_empty()
    }
}

impl<L> Layer for Temporal<'_, '_, L> where L: Layer {}

impl<L> ReadLayer for Temporal<'_, '_, L>
where
    L: ReadLayer,
{
    fn has<K: AsKeyParts>(&self, key: &K) -> EyreResult<bool> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(false),
            Some(Operation::Put { .. }) => Ok(true),
            None => self.inner.has(key),
        }
    }

    fn get<K: AsKeyParts>(&self, key: &K) -> EyreResult<Option<Slice<'_>>> {
        match self.shadow.get(key) {
            Some(Operation::Delete) => Ok(None),
            Some(Operation::Put { value }) => Ok(Some(value.into())),
            None => self.inner.get(key),
        }
    }

    fn iter<K: FromKeyParts>(&self) -> EyreResult<Iter<'_, Structured<K>>> {
        Ok(Iter::new(TemporalIterator {
            inner: self.inner.iter::<K>()?,
            inner_done: false,
            shadow_done: false,
            shadow: &self.shadow,
            shadow_iter: None,
            peeked_inner: None,
            peeked_shadow: None,
            value: None,
        }))
    }

    fn iter_snapshot<K: FromKeyParts>(&self) -> EyreResult<Iter<'_, Structured<K>>> {
        // For temporal layer, snapshot iteration still needs to consider
        // the shadow transaction, so we use the same logic as iter()
        // but with a snapshot iterator for the underlying layer
        Ok(Iter::new(TemporalIterator {
            inner: self.inner.iter_snapshot::<K>()?,
            inner_done: false,
            shadow_done: false,
            shadow: &self.shadow,
            shadow_iter: None,
            peeked_inner: None,
            peeked_shadow: None,
            value: None,
        }))
    }
}

impl<'entry, L> WriteLayer<'entry> for Temporal<'_, 'entry, L>
where
    L: WriteLayer<'entry>,
{
    fn put<K: AsKeyParts>(&mut self, key: &'entry K, value: Slice<'entry>) -> EyreResult<()> {
        self.shadow.put(key, value);

        Ok(())
    }

    fn delete<K: AsKeyParts>(&mut self, key: &'entry K) -> EyreResult<()> {
        self.shadow.delete(key);

        Ok(())
    }

    fn apply(&mut self, tx: &Transaction<'entry>) -> EyreResult<()> {
        self.shadow.merge(tx);

        Ok(())
    }

    fn commit(&mut self) -> EyreResult<()> {
        self.inner.apply(&self.shadow)?;

        Ok(())
    }
}

struct TemporalIterator<'a, 'b, K> {
    inner: Iter<'a, Structured<K>>,
    /// Set once `inner` is exhausted so we stop polling it.
    inner_done: bool,
    /// Set once the shadow column range is exhausted so we stop polling it.
    shadow_done: bool,
    shadow: &'a Transaction<'b>,
    shadow_iter: Option<tx::ColRange<'a, 'b>>,
    /// Buffered front key of `inner`. Owned (copied) so it doesn't borrow
    /// `inner`, letting us hold it across the merge while `inner` stays parked
    /// on the same record (so [`read`](Self::read) can still fetch its value).
    peeked_inner: Option<Slice<'a>>,
    /// Buffered front entry of the shadow column range.
    peeked_shadow: Option<(Slice<'a>, &'a Operation<'b>)>,
    value: Option<Slice<'a>>,
}

impl<'a, K: AsKeyParts + FromKeyParts> TemporalIterator<'a, '_, K> {
    /// Produce the next key in the merged, sorted ordering of the underlying
    /// layer (`inner`) and the pending shadow transaction.
    ///
    /// Both sources are individually sorted by raw key bytes, so this is a
    /// two-way merge: the smaller of the two front keys is emitted, and when
    /// the same key is present in both, the shadow entry takes precedence (and
    /// the duplicate inner key is dropped). Shadow deletions are skipped.
    fn advance(&mut self) -> EyreResult<Option<Slice<'_>>> {
        loop {
            // Refill the inner front key. We copy it into an owned `Slice` so
            // we can keep it buffered without borrowing `inner`; `inner` itself
            // stays positioned on this record until we next refill, which keeps
            // `read()` valid for an inner-sourced key.
            if !self.inner_done && self.peeked_inner.is_none() {
                match self.inner.next()? {
                    Some(key) => self.peeked_inner = Some(key.into_boxed().into()),
                    None => self.inner_done = true,
                }
            }

            // Refill the shadow front entry. `shadow_iter` is created lazily on
            // the first `next()` call (unbounded start); `seek()` pre-creates it
            // with a start bound. Once it runs dry we set `shadow_done` so we
            // stop polling it, mirroring `inner_done`.
            if !self.shadow_done && self.peeked_shadow.is_none() {
                let shadow_iter = self
                    .shadow_iter
                    .get_or_insert_with(|| self.shadow.col_iter(K::column(), None));

                match shadow_iter.next() {
                    Some(entry) => self.peeked_shadow = Some(entry),
                    None => self.shadow_done = true,
                }
            }

            // Compare the two fronts. The synthetic orderings make a missing
            // side compare as "greater", so the present side is emitted.
            let order = match (self.peeked_inner.as_ref(), self.peeked_shadow.as_ref()) {
                (None, None) => return Ok(None),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(inner_key), Some((shadow_key, _))) => inner_key.cmp(shadow_key),
            };

            match order {
                // Inner key comes first and isn't shadowed: emit it. We leave
                // `value` as `None` and read it lazily from `inner` in `read()`.
                // This is sound because `inner` stays parked on this record
                // until the next `advance()` refills `peeked_inner` (the first
                // thing the loop does). `value == None` is therefore the
                // invariant marking "the last emitted key was inner-sourced and
                // `inner` is still positioned on it"; any shadow-sourced key
                // sets `value` to `Some` instead, so `read()` never delegates to
                // a stale `inner` position.
                Ordering::Less => {
                    self.value = None;

                    return Ok(self.peeked_inner.take());
                }
                // Same key in both: the shadow entry wins, so drop the
                // duplicate inner key and fall through to emit the shadow one.
                Ordering::Equal => self.peeked_inner = None,
                // Shadow key comes first: fall through to emit it.
                Ordering::Greater => {}
            }

            let (key, op) = self.peeked_shadow.take().expect("shadow front present");

            match op {
                Operation::Delete => continue,
                Operation::Put { value } => {
                    self.value = Some(value.into());

                    return Ok(Some(key));
                }
            }
        }
    }
}

impl<'a, K: AsKeyParts + FromKeyParts> DBIter for TemporalIterator<'a, '_, K> {
    /// Seeks to the first *visible* key `>= key` in the merged ordering. A key
    /// that exists in the shadow as a deletion is skipped, so the returned key
    /// may be strictly greater than `key` even when `key` is present in `inner`.
    fn seek(&mut self, key: Slice<'_>) -> EyreResult<Option<Slice<'_>>> {
        self.value = None;
        self.peeked_inner = None;
        self.peeked_shadow = None;
        self.inner_done = false;
        self.shadow_done = false;
        self.shadow_iter = Some(self.shadow.col_iter(K::column(), Some(&key)));

        match self.inner.seek(key)? {
            Some(inner_key) => self.peeked_inner = Some(inner_key.into_boxed().into()),
            None => self.inner_done = true,
        }

        self.advance()
    }

    fn next(&mut self) -> EyreResult<Option<Slice<'_>>> {
        self.advance()
    }

    fn read(&self) -> EyreResult<Slice<'_>> {
        // A buffered `value` means the last emitted key was shadow-sourced (a
        // `Put`); otherwise the last key was inner-sourced and `inner` is still
        // parked on it, so we delegate. See the `Ordering::Less` arm in
        // `advance()` for the invariant this relies on.
        if let Some(value) = &self.value {
            return Ok(value.into());
        }

        self.inner.read()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::db::InMemoryDB;
    use crate::iter::DBIter;
    use crate::key::Generic;
    use crate::layer::{LayerExt, ReadLayer, WriteLayer};
    use crate::slice::Slice;
    use crate::Store;

    // A `Generic` key whose scope and fragment bytes are all `id`, so the raw
    // key bytes sort by `id` and `key[0]` recovers it.
    fn key(id: u8) -> Generic {
        Generic::new([id; 16], [id; 32])
    }

    fn val(bytes: &'static [u8]) -> Slice<'static> {
        Slice::from(bytes)
    }

    // Drive the iterator at the raw `DBIter` level (the merge operates on raw
    // bytes; this also sidesteps `Generic`'s non-`Error` key error type).
    fn collect<L: ReadLayer>(layer: &L) -> Vec<(u8, Vec<u8>)> {
        let mut iter = layer.iter::<Generic>().expect("iter");
        let mut out = Vec::new();

        loop {
            let id = match DBIter::next(&mut iter).expect("next") {
                Some(key) => key[0],
                None => break,
            };
            let value = DBIter::read(&iter).expect("read").as_ref().to_vec();
            out.push((id, value));
        }

        out
    }

    fn collect_from<L: ReadLayer>(layer: &L, start: u8) -> Vec<(u8, Vec<u8>)> {
        let mut iter = layer.iter::<Generic>().expect("iter");
        let mut out = Vec::new();

        let seek = [start; 48];

        let mut id = DBIter::seek(&mut iter, Slice::from(&seek[..]))
            .expect("seek")
            .map(|key| key[0]);

        while let Some(scope) = id {
            let value = DBIter::read(&iter).expect("read").as_ref().to_vec();
            out.push((scope, value));

            id = DBIter::next(&mut iter).expect("next").map(|key| key[0]);
        }

        out
    }

    #[test]
    fn merges_inner_and_shadow_in_sorted_order() {
        let mut store = Store::new(Arc::new(InMemoryDB::owned()));

        // Committed base layer holds scopes 1, 3, 5.
        for id in [1_u8, 3, 5] {
            let k = key(id);
            store.put(&k, val(b"base")).expect("base put");
        }

        // Pending transaction inserts 2 and 6, overrides 3, deletes 5.
        let (k2, k3, k5, k6) = (key(2), key(3), key(5), key(6));

        let mut temporal = store.temporal();
        temporal.put(&k2, val(b"new")).expect("put 2");
        temporal.put(&k3, val(b"override")).expect("put 3");
        temporal.delete(&k5).expect("delete 5");
        temporal.put(&k6, val(b"new")).expect("put 6");

        assert_eq!(
            collect(&temporal),
            vec![
                (1, b"base".to_vec()),
                (2, b"new".to_vec()),
                (3, b"override".to_vec()),
                (6, b"new".to_vec()),
            ],
        );
    }

    #[test]
    fn shadow_only_keys_are_yielded_in_order() {
        let mut store = Store::new(Arc::new(InMemoryDB::owned()));

        let (k1, k2, k3) = (key(1), key(2), key(3));

        let mut temporal = store.temporal();
        // Insert out of order to confirm the merge sorts, not insertion order.
        temporal.put(&k3, val(b"c")).expect("put 3");
        temporal.put(&k1, val(b"a")).expect("put 1");
        temporal.put(&k2, val(b"b")).expect("put 2");

        assert_eq!(
            collect(&temporal),
            vec![(1, b"a".to_vec()), (2, b"b".to_vec()), (3, b"c".to_vec()),],
        );
    }

    #[test]
    fn each_key_is_yielded_once_when_present_in_both() {
        let mut store = Store::new(Arc::new(InMemoryDB::owned()));

        for id in 1_u8..=4 {
            let k = key(id);
            store.put(&k, val(b"base")).expect("base put");
        }

        // Override every committed key in the shadow transaction.
        let keys: Vec<Generic> = (1_u8..=4).map(key).collect();

        let mut temporal = store.temporal();
        for k in &keys {
            temporal.put(k, val(b"override")).expect("override put");
        }

        assert_eq!(
            collect(&temporal),
            vec![
                (1, b"override".to_vec()),
                (2, b"override".to_vec()),
                (3, b"override".to_vec()),
                (4, b"override".to_vec()),
            ],
        );
    }

    #[test]
    fn seek_resumes_the_merge_from_the_target_key() {
        let mut store = Store::new(Arc::new(InMemoryDB::owned()));

        for id in [1_u8, 3, 5] {
            let k = key(id);
            store.put(&k, val(b"base")).expect("base put");
        }

        let (k3, k5, k6) = (key(3), key(5), key(6));

        let mut temporal = store.temporal();
        temporal.put(&k3, val(b"override")).expect("put 3");
        temporal.delete(&k5).expect("delete 5");
        temporal.put(&k6, val(b"new")).expect("put 6");

        // Seeking to 3 should skip 1 and resume the merge (override 3, the
        // deleted 5 is gone, then the shadow-only 6).
        assert_eq!(
            collect_from(&temporal, 3),
            vec![(3, b"override".to_vec()), (6, b"new".to_vec())],
        );
    }
}
