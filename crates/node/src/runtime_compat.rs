use core::cell::RefCell;
use core::mem::transmute;
use std::sync::Arc;

use calimero_primitives::context::ContextId;
use calimero_runtime::store::{Key, Storage, Value};
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::layer::temporal::Temporal;
use calimero_store::layer::{LayerExt, ReadLayer, WriteLayer};
use calimero_store::Store;
use eyre::Result as EyreResult;

#[derive(Debug)]
pub struct RuntimeCompatStore<'this, 'entry> {
    context_id: ContextId,
    inner: Temporal<'this, 'entry, Store>,
    // todo! unideal, will revisit the shape of WriteLayer to own keys (since they are now fixed-sized)
    keys: RefCell<Vec<Arc<ContextStateKey>>>,
}

impl<'this, 'entry> RuntimeCompatStore<'this, 'entry> {
    pub fn new(store: &'this mut Store, context_id: ContextId) -> Self {
        Self {
            context_id,
            inner: store.temporal(),
            keys: RefCell::default(),
        }
    }

    fn state_key(&self, key: &[u8]) -> Option<&'entry ContextStateKey> {
        let mut state_key = [0; 32];

        (key.len() <= state_key.len()).then_some(())?;

        state_key[..key.len()].copy_from_slice(key);

        let mut keys = self.keys.borrow_mut();

        keys.push(Arc::new(ContextStateKey::new(self.context_id, state_key)));

        // safety: TemporalStore lives as long as Self, so the reference will hold
        unsafe {
            transmute::<Option<&ContextStateKey>, Option<&'entry ContextStateKey>>(
                keys.last().map(|x| &**x),
            )
        }
    }

    pub fn commit(self) -> EyreResult<()> {
        self.inner.commit()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl Storage for RuntimeCompatStore<'_, '_> {
    fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        let slice = self.inner.get(key).ok()??;

        Some(slice.into_boxed().into_vec())
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        let old = self
            .inner
            .get(key)
            .ok()
            .flatten()
            .map(|slice| slice.into_boxed().into_vec());

        self.inner.delete(key).ok()?;

        old
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        let key = self.state_key(&key)?;

        let old = self
            .inner
            .has(key)
            .ok()?
            .then(|| self.inner.get(key).ok().flatten())
            .flatten()
            .map(|slice| slice.into_boxed().into_vec());

        self.inner.put(key, value.into()).ok()?;

        old
    }

    fn has(&self, key: &Key) -> bool {
        let Some(key) = self.state_key(key) else {
            return false;
        };

        self.inner.has(key).unwrap_or(false)
    }
}
