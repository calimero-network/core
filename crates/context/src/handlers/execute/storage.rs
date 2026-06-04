#![allow(clippy::mem_forget, reason = "ouroboros uses it")]

use core::cell::RefCell;
use core::mem;
use std::sync::Arc;

use calimero_primitives::context::ContextId;
use calimero_runtime::store::{Key, Storage, Value};
use calimero_store::db::Column;
use calimero_store::layer::temporal::Temporal;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::{key, Store};
use ouroboros::self_referencing;

#[self_referencing]
pub struct ContextStorage {
    context_id: ContextId,
    store: Store,
    /// Second handle to the same DB (cheap — the backend is `Arc`-shared) for
    /// the node-local ordered secondary index (`SortedMap`, core#2559). The
    /// synced `store` above is borrowed by `inner` (the temporal transaction),
    /// so the index — which lives in a *separate*, non-synced column and is
    /// written immediately rather than through the transaction — needs its own
    /// handle.
    index_store: Store,

    #[covariant]
    #[borrows(mut store)]
    inner: Temporal<'this, 'static, Store>,
    // todo! unideal, will revisit the shape of WriteLayer to own keys (since they are now fixed-sized)
    keys: RefCell<Vec<Arc<key::ContextState>>>,
}

/// The exclusive upper bound for a byte prefix (smallest key not starting with
/// `prefix`) — used to scan/clear "all keys under this prefix".
fn prefix_upper_bound(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(&last) = end.last() {
        if last == 0xFF {
            let _ = end.pop();
        } else {
            *end.last_mut().expect("non-empty") += 1;
            return end;
        }
    }
    vec![0xFF; prefix.len() + 1]
}

/// Node-local private storage that is NOT synchronized across nodes.
/// This uses a separate database column (`PrivateState`) so that private data
/// never appears in sync deltas.
#[self_referencing]
pub struct ContextPrivateStorage {
    context_id: ContextId,
    store: Store,

    #[covariant]
    #[borrows(mut store)]
    inner: Temporal<'this, 'static, Store>,
    keys: RefCell<Vec<Arc<key::ContextPrivateState>>>,
}

// safety: ContextStorage is constructed exclusively for the runtime
//         which maintains exclusive access in a single-threaded environment
//         before returning the same instance back to the constructor
//         which then proceeds to directly commit any written data
//         never having multiple references to this same instance
//         --
//         we can eventually get rid of this when Slice<'_>: Send
//         ref: https://github.com/calimero-network/core/commit/455fe09ca9be09df17046584a3ef6cd28564e01a
unsafe impl Send for ContextStorage {}

impl ContextStorage {
    pub fn from(store: Store, context_id: ContextId) -> Self {
        let index_store = store.clone();
        ContextStorageBuilder {
            context_id,
            index_store,
            store,
            inner_builder: |store| Temporal::new(store),
            keys: RefCell::default(),
        }
        .build()
    }

    /// Byte length of the context-id prefix stamped onto every index key.
    /// Derived from the live id (not hardcoded) so the strip on readback stays
    /// in lockstep with [`Self::index_key`].
    fn index_prefix_len(&self) -> usize {
        self.borrow_context_id().as_ref().len()
    }

    /// Scope an ordered-index key to this context: `context_id ‖ key`. The
    /// `key` is the adaptor's `collection ‖ order_key`; prefixing the context
    /// keeps different contexts' indexes disjoint in the shared column.
    fn index_key(&self, key: &[u8]) -> Vec<u8> {
        let context = self.borrow_context_id();
        let context = context.as_ref();
        let mut out = Vec::with_capacity(context.len() + key.len());
        out.extend_from_slice(context);
        out.extend_from_slice(key);
        out
    }

    fn state_key(&self, key: &[u8]) -> Option<&'static key::ContextState> {
        let mut state_key = [0; 32];

        (key.len() <= state_key.len()).then_some(())?;

        state_key[..key.len()].copy_from_slice(key);

        let mut keys = self.borrow_keys().borrow_mut();

        let context_id = self.borrow_context_id();

        keys.push(Arc::new(key::ContextState::new(*context_id, state_key)));

        // safety: TemporalStore lives as long as Self, so the reference will hold
        //         plus, we never return a reference to the keys externally
        unsafe {
            mem::transmute::<Option<&key::ContextState>, Option<&'static key::ContextState>>(
                keys.last().map(|x| &**x),
            )
        }
    }

    pub fn commit(mut self) -> eyre::Result<Store> {
        self.with_inner_mut(|inner| inner.commit())?;

        Ok(self.into_heads().store)
    }

    pub fn is_empty(&self) -> bool {
        self.borrow_inner().is_empty()
    }
}

impl Storage for ContextStorage {
    fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        let slice = self.borrow_inner().get(key).ok()??;

        Some(slice.into_boxed().into_vec())
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        self.with_inner_mut(|inner| {
            let old = inner
                .get(key)
                .ok()
                .flatten()
                .map(|slice| slice.into_boxed().into_vec());

            inner.delete(key).ok()?;

            old
        })
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        let key = self.state_key(&key)?;

        self.with_inner_mut(|inner| {
            let old = inner
                .has(key)
                .ok()?
                .then(|| inner.get(key).ok().flatten())
                .flatten()
                .map(|slice| slice.into_boxed().into_vec());

            inner.put(key, value.into()).ok()?;

            old
        })
    }

    fn has(&self, key: &Key) -> bool {
        let Some(key) = self.state_key(key) else {
            return false;
        };

        self.borrow_inner().has(key).unwrap_or_default()
    }

    // Ordered secondary index (SortedMap, core#2559) — node-local, written
    // straight to the non-synced `Column::SortedIndex` via the second store
    // handle (not the temporal transaction). Keys are `context_id ‖ collection
    // ‖ order_key` (all unhashed), so RocksDB's byte order is the key order.

    fn index_set(&mut self, key: &[u8], value: &[u8]) -> bool {
        let full = self.index_key(key);
        self.borrow_index_store()
            .raw_put(Column::SortedIndex, &full, value)
            .is_ok()
    }

    fn index_del(&mut self, key: &[u8]) -> bool {
        let full = self.index_key(key);
        self.borrow_index_store()
            .raw_delete(Column::SortedIndex, &full)
            .is_ok()
    }

    fn index_del_prefix(&mut self, prefix: &[u8]) -> bool {
        let lo = self.index_key(prefix);
        let hi = prefix_upper_bound(&lo);
        // One range tombstone over the prefix slice — no scan, no per-key
        // delete, no unbounded buffer of the collection's keys.
        self.borrow_index_store()
            .raw_delete_range(Column::SortedIndex, &lo, &hi)
            .is_ok()
    }

    fn index_scan(
        &self,
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let full_lo = self.index_key(lo);
        let full_hi = self.index_key(hi);
        // Stop the seek after `offset + limit` items so a bounded read walks
        // O(offset + limit), not the whole range.
        let max = limit.map(|n| offset.saturating_add(n));
        // A scan error must not masquerade as "no entries": log it loudly so a
        // backend fault is visible rather than silently dropping rows. The
        // authoritative entity set is untouched, and the next ordered read
        // rebuilds the index via the validity marker, so an empty page here is a
        // recoverable degradation, not data loss.
        let pairs = match self.borrow_index_store().raw_scan(
            Column::SortedIndex,
            &full_lo,
            &full_hi,
            max,
        ) {
            Ok(pairs) => pairs,
            Err(error) => {
                tracing::error!(
                    target: "calimero_context::sorted_index",
                    %error,
                    "ordered-index scan failed; returning an empty page (entities intact, index self-heals on next read)"
                );
                Vec::new()
            }
        };
        // Strip the context-id prefix to hand back the adaptor-level key.
        let plen = self.index_prefix_len();
        let stripped = pairs
            .into_iter()
            .filter_map(|(k, v)| k.get(plen..).map(|key| (key.to_vec(), v)))
            .skip(offset);
        match limit {
            Some(n) => stripped.take(n).collect(),
            None => stripped.collect(),
        }
    }

    fn index_last(&self, lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let full_lo = self.index_key(lo);
        let full_hi = self.index_key(hi);
        let (k, v) = self
            .borrow_index_store()
            .raw_last(Column::SortedIndex, &full_lo, &full_hi)
            .ok()??;
        // Strip the context-id prefix.
        Some((k.get(self.index_prefix_len()..)?.to_vec(), v))
    }
}

// Same safety reasoning as ContextStorage
unsafe impl Send for ContextPrivateStorage {}

impl ContextPrivateStorage {
    pub fn from(store: Store, context_id: ContextId) -> Self {
        ContextPrivateStorageBuilder {
            context_id,
            store,
            inner_builder: |store| Temporal::new(store),
            keys: RefCell::default(),
        }
        .build()
    }

    fn state_key(&self, key: &[u8]) -> Option<&'static key::ContextPrivateState> {
        let mut state_key = [0; 32];

        (key.len() <= state_key.len()).then_some(())?;

        state_key[..key.len()].copy_from_slice(key);

        let mut keys = self.borrow_keys().borrow_mut();

        let context_id = self.borrow_context_id();

        keys.push(Arc::new(key::ContextPrivateState::new(
            *context_id,
            state_key,
        )));

        // safety: TemporalStore lives as long as Self, so the reference will hold
        //         plus, we never return a reference to the keys externally
        unsafe {
            mem::transmute::<
                Option<&key::ContextPrivateState>,
                Option<&'static key::ContextPrivateState>,
            >(keys.last().map(|x| &**x))
        }
    }

    pub fn commit(mut self) -> eyre::Result<Store> {
        self.with_inner_mut(|inner| inner.commit())?;

        Ok(self.into_heads().store)
    }
}

impl Storage for ContextPrivateStorage {
    fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        let slice = self.borrow_inner().get(key).ok()??;

        Some(slice.into_boxed().into_vec())
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        self.with_inner_mut(|inner| {
            let old = inner
                .get(key)
                .ok()
                .flatten()
                .map(|slice| slice.into_boxed().into_vec());

            inner.delete(key).ok()?;

            old
        })
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        let key = self.state_key(&key)?;

        self.with_inner_mut(|inner| {
            let old = inner
                .has(key)
                .ok()?
                .then(|| inner.get(key).ok().flatten())
                .flatten()
                .map(|slice| slice.into_boxed().into_vec());

            inner.put(key, value.into()).ok()?;

            old
        })
    }

    fn has(&self, key: &Key) -> bool {
        let Some(key) = self.state_key(key) else {
            return false;
        };

        self.borrow_inner().has(key).unwrap_or_default()
    }
}

/// A read-only view over a [`ContextStorage`] or [`ContextPrivateStorage`].
///
/// Passed to [`calimero_runtime::Module::run`] in place of the normal mutable
/// storage when the executing method holds a *shared* read guard on the
/// per-context `RwLock`. The write methods (`set`, `remove`, `index_set`,
/// `index_del`, `index_del_prefix`) return their "nothing happened" values so
/// that storage-level WASM host calls fail gracefully rather than panicking.
/// A post-execution assertion on `outcome.artifact` / `outcome.root_hash`
/// catches any method that nonetheless tried to write and treats it as an error.
pub struct ReadOnlyContextStorage<'a, S>(&'a mut S);

impl<'a, S: Storage> ReadOnlyContextStorage<'a, S> {
    pub fn new(inner: &'a mut S) -> Self {
        Self(inner)
    }
}

impl<S: Storage> Storage for ReadOnlyContextStorage<'_, S> {
    fn get(&self, key: &Key) -> Option<Value> {
        self.0.get(key)
    }

    fn has(&self, key: &Key) -> bool {
        self.0.has(key)
    }

    fn index_scan(
        &self,
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.0.index_scan(lo, hi, offset, limit)
    }

    fn index_last(&self, lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        self.0.index_last(lo, hi)
    }

    // Write methods return their "nothing written" values — a read-only execution
    // should produce an empty artifact; the post-exec assertion catches violations.
    fn set(&mut self, _key: Key, _value: Value) -> Option<Value> {
        None
    }

    fn remove(&mut self, _key: &Key) -> Option<Value> {
        None
    }

    fn index_set(&mut self, _key: &[u8], _value: &[u8]) -> bool {
        false
    }

    fn index_del(&mut self, _key: &[u8]) -> bool {
        false
    }

    fn index_del_prefix(&mut self, _prefix: &[u8]) -> bool {
        false
    }
}
