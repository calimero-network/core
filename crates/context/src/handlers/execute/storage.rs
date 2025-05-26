#![allow(clippy::mem_forget, reason = "ouroboros uses it")]

use core::cell::RefCell;
use core::mem;
use std::sync::Arc;

use calimero_primitives::context::ContextId;
use calimero_runtime::store::{Key, Storage, Value};
use calimero_store::layer::temporal::Temporal;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::{key, Store};
use ouroboros::self_referencing;

#[self_referencing]
pub struct ContextStorage {
    context_id: ContextId,
    store: Store,

    #[covariant]
    #[borrows(mut store)]
    inner: Temporal<'this, 'static, Store>,
    // todo! unideal, will revisit the shape of WriteLayer to own keys (since they are now fixed-sized)
    keys: RefCell<Vec<Arc<key::ContextState>>>,
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
        ContextStorageBuilder {
            context_id,
            store,
            inner_builder: |store| Temporal::new(store),
            keys: RefCell::default(),
        }
        .build()
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
}
