use core::cell::RefCell;
use core::mem::transmute;

use calimero_primitives::context::ContextId;
use calimero_runtime::store::{Key, Storage, Value};
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::layer::read_only::ReadOnly;
use calimero_store::layer::temporal::Temporal;
use calimero_store::layer::{LayerExt, ReadLayer, WriteLayer};
use calimero_store::Store;
use eyre::Result as EyreResult;

#[derive(Debug)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum RuntimeCompatStoreInner<'this, 'entry> {
    Read(ReadOnly<'this, Store>),
    Write(Temporal<'this, 'entry, Store>),
}

#[derive(Debug)]
pub struct RuntimeCompatStore<'this, 'entry> {
    context_id: ContextId,
    inner: RuntimeCompatStoreInner<'this, 'entry>,
    // todo! unideal, will revisit the shape of WriteLayer to own keys (since they are now fixed-sized)
    keys: RefCell<Vec<ContextStateKey>>,
}

impl<'this, 'entry> RuntimeCompatStore<'this, 'entry> {
    pub fn temporal(store: &'this mut Store, context_id: ContextId) -> Self {
        Self {
            context_id,
            inner: RuntimeCompatStoreInner::Write(store.temporal()),
            keys: RefCell::default(),
        }
    }

    #[must_use]
    pub fn read_only(store: &'this Store, context_id: ContextId) -> Self {
        Self {
            context_id,
            inner: RuntimeCompatStoreInner::Read(store.read_only()),
            keys: RefCell::default(),
        }
    }

    fn state_key(&self, key: &[u8]) -> Option<&'entry ContextStateKey> {
        let mut state_key = [0; 32];

        (key.len() <= state_key.len()).then_some(())?;

        state_key[..key.len()].copy_from_slice(key);

        let mut keys = self.keys.borrow_mut();

        keys.push(ContextStateKey::new(self.context_id, state_key));

        // safety: TemporalStore lives as long as Self, so the reference will hold
        unsafe {
            transmute::<Option<&ContextStateKey>, Option<&'entry ContextStateKey>>(keys.last())
        }
    }

    pub fn commit(self) -> EyreResult<bool> {
        if let RuntimeCompatStoreInner::Write(store) = self.inner {
            return store.commit().and(Ok(true));
        }

        Ok(false)
    }
}

impl Storage for RuntimeCompatStore<'_, '_> {
    fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        let maybe_slice = match &self.inner {
            RuntimeCompatStoreInner::Read(store) => store.get(key),
            RuntimeCompatStoreInner::Write(store) => store.get(key),
        };

        let slice = maybe_slice.ok()??;

        Some(slice.into_boxed().into_vec())
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        let key = self.state_key(&key)?;

        let RuntimeCompatStoreInner::Write(store) = &mut self.inner else {
            unimplemented!("Can not write to read-only store.");
        };

        let old = store
            .has(key)
            .ok()?
            .then(|| store.get(key).ok().flatten())
            .flatten()
            .map(|slice| slice.into_boxed().into_vec());

        store.put(key, value.into()).ok()?;

        old
    }

    fn has(&self, key: &Key) -> bool {
        let Some(key) = self.state_key(key) else {
            return false;
        };

        match &self.inner {
            RuntimeCompatStoreInner::Read(store) => store.has(key),
            RuntimeCompatStoreInner::Write(store) => store.has(key),
        }
        .ok()
        .unwrap_or(false)
    }
}
