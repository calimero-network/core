use std::cell::RefCell;

use calimero_primitives::context::ContextId;
use calimero_store::key::ContextState;
use calimero_store::layer::{read_only, temporal, LayerExt, ReadLayer, WriteLayer};
use calimero_store::Store;

pub enum RuntimeCompatStoreInner<'a, 'k, 'v> {
    Read(read_only::ReadOnly<'k, Store>),
    Write(temporal::Temporal<'a, 'k, 'v, Store>),
}

pub struct RuntimeCompatStore<'a, 'k, 'v> {
    context_id: ContextId,
    inner: RuntimeCompatStoreInner<'a, 'k, 'v>,
    // todo! unideal, will revisit the shape of WriteLayer to own keys (since they are now fixed-sized)
    keys: RefCell<Vec<ContextState>>,
}

impl<'a, 'k, 'v> RuntimeCompatStore<'a, 'k, 'v> {
    pub fn temporal(store: &'a mut Store, context_id: ContextId) -> Self {
        Self {
            context_id,
            inner: RuntimeCompatStoreInner::Write(store.temporal()),
            keys: Default::default(),
        }
    }

    pub fn read_only(store: &'k Store, context_id: ContextId) -> Self {
        Self {
            context_id,
            inner: RuntimeCompatStoreInner::Read(store.read_only()),
            keys: Default::default(),
        }
    }

    fn state_key(&self, key: &[u8]) -> Option<&'k ContextState> {
        let mut state_key = [0; 32];

        (key.len() <= state_key.len()).then_some(())?;

        (&mut state_key[..key.len()]).copy_from_slice(key);

        let mut keys = self.keys.borrow_mut();

        keys.push(ContextState::new(self.context_id, state_key));

        // safety: TemporalStore lives as long as Self, so the reference will hold
        unsafe {
            std::mem::transmute::<Option<&ContextState>, Option<&'k ContextState>>(keys.last())
        }
    }

    pub fn commit(self) -> eyre::Result<bool> {
        if let RuntimeCompatStoreInner::Write(store) = self.inner {
            return store.commit().and(Ok(true));
        }

        Ok(false)
    }
}

impl<'a, 'k, 'v> calimero_runtime::store::Storage for RuntimeCompatStore<'a, 'k, 'v> {
    fn get(&self, key: &calimero_runtime::store::Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        let maybe_slice = match &self.inner {
            RuntimeCompatStoreInner::Read(store) => store.get(key),
            RuntimeCompatStoreInner::Write(store) => store.get(key),
        };

        let slice = maybe_slice.ok()??;

        Some(slice.into_boxed().into_vec())
    }

    fn set(
        &mut self,
        key: calimero_runtime::store::Key,
        value: calimero_runtime::store::Value,
    ) -> Option<calimero_runtime::store::Value> {
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

    fn has(&self, key: &calimero_runtime::store::Key) -> bool {
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
