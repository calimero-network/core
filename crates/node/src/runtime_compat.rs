use core::cell::RefCell;
use core::mem;
use std::collections::HashSet;

use calimero_primitives::context::ContextId;
use calimero_runtime::store::{Id, Storage, StorageError, Value};
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
    keys: RefCell<HashSet<ContextStateKey>>,
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

    fn state_key(&self, key: Id) -> &'entry ContextStateKey {
        let mut keys = self.keys.borrow_mut();

        let key = ContextStateKey::new(self.context_id, key);

        let _ = keys.insert(key);

        // safety: TemporalStore lives as long as Self, so the reference will hold
        unsafe {
            mem::transmute::<&ContextStateKey, &'entry ContextStateKey>(
                keys.get(&key).expect("we just pushed"),
            )
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
    fn create(&mut self, is_collection: bool) -> Result<Id, StorageError> {
        todo!()
    }

    fn exists(&self, key: &Id) -> Result<bool, StorageError> {
        let key = self.state_key(*key);

        match &self.inner {
            RuntimeCompatStoreInner::Read(store) => store.has(key),
            RuntimeCompatStoreInner::Write(store) => store.has(key),
        }
        .map_err(|e| StorageError::Other(e.into()))
    }

    fn read(&self, key: &Id) -> Result<Option<Value>, StorageError> {
        let key = self.state_key(*key);

        let maybe_slice = match &self.inner {
            RuntimeCompatStoreInner::Read(store) => store.get(key),
            RuntimeCompatStoreInner::Write(store) => store.get(key),
        };

        let Some(slice) = maybe_slice.map_err(|e| StorageError::Other(e.into()))? else {
            return Ok(None);
        };

        Ok(Some(slice.into_boxed().into_vec()))
    }

    fn write(&mut self, key: Id, value: Value) -> Result<Option<Value>, StorageError> {
        let key = self.state_key(key);

        let RuntimeCompatStoreInner::Write(store) = &mut self.inner else {
            unimplemented!("Can not write to read-only store.");
        };

        let old = store
            .get(key)
            .map_err(|e| StorageError::Other(e.into()))?
            .map(|slice| slice.into_boxed().into_vec());

        store
            .put(key, value.into())
            .map_err(|e| StorageError::Other(e.into()))?;

        Ok(old)
    }

    fn remove(&mut self, key: &Id) -> Result<Option<Value>, StorageError> {
        todo!()
    }

    fn adopt(&mut self, key: Id, parent: &Id) -> Result<bool, StorageError> {
        todo!()
    }

    fn orphan(&mut self, key: Id, parent: &Id) -> Result<bool, StorageError> {
        todo!()
    }
}
