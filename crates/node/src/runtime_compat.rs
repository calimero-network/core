use core::cell::RefCell;
use std::sync::Arc;

use calimero_primitives::context::ContextId;
use calimero_runtime::store::{Key, Storage, Value};
use calimero_store::key::ContextState as ContextStateKey;
use calimero_store::layer::temporal::Temporal;
use calimero_store::layer::{ReadLayer, WriteLayer};
use calimero_store::Store;
use eyre::Result as EyreResult;
use ouroboros::self_referencing;

pub struct RuntimeStore {
    pub inner: RuntimeCompatStore,
}

impl RuntimeStore {
    pub fn new(store: Store, context_id: ContextId) -> Self {
        RuntimeStore {
            inner: RuntimeCompatStoreBuilder {
                context_id,
                store,
                inner_builder: |store| Temporal::new(store),
                keys: RefCell::default(),
            }
            .build(),
        }
    }
}
#[self_referencing]
pub struct RuntimeCompatStore {
    context_id: ContextId,
    store: Store,

    #[covariant]
    #[borrows(mut store)]
    inner: Temporal<'this, 'static, Store>,
    // todo! unideal, will revisit the shape of WriteLayer to own keys (since they are now fixed-sized)
    keys: RefCell<Vec<Arc<ContextStateKey>>>,
}

unsafe impl Send for RuntimeCompatStore {}

impl RuntimeCompatStore {
    fn state_key(&self, key: &[u8]) -> Option<ContextStateKey> {
        let mut state_key = [0; 32];

        (key.len() <= state_key.len()).then_some(())?;

        state_key[..key.len()].copy_from_slice(key);

        let mut keys = self.borrow_keys().borrow_mut();

        keys.push(Arc::new(ContextStateKey::new(
            *self.borrow_context_id(),
            state_key,
        )));

        // safety: TemporalStore lives as long as Self, so the reference will hold

        keys.last().map(|x| *&**x) //?
    }

    pub fn commit(mut self) -> EyreResult<()> {
        self.with_inner_mut(|inner| inner.commit())
    }

    pub fn is_empty(&self) -> bool {
        self.borrow_inner().is_empty()
    }
}

impl Storage for RuntimeCompatStore {
    fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        self.with_inner(|inner| {
            let slice = inner.get(&key).ok()??;
            Some(slice.into_boxed().into_vec())
        })
    }

    fn remove(&mut self, key: &Key) -> Option<Vec<u8>> {
        let key = self.state_key(key)?;

        self.with_inner_mut(|inner| {
            let old = inner
                .get(&key)
                .ok()
                .flatten()
                .map(|slice| slice.into_boxed().into_vec());

            inner.delete(&key).ok()?;

            old
        })
    }

    fn set(&mut self, key: Key, value: Value) -> Option<Value> {
        let key = self.state_key(&key)?;

        self.with_inner_mut(|inner| {
            let old = inner
                .has(&key)
                .ok()?
                .then(|| inner.get(&key).ok().flatten())
                .flatten()
                .map(|slice| slice.into_boxed().into_vec());

            inner.put(&key, value.into()).ok()?;

            old
        })
    }

    fn has(&self, key: &Key) -> bool {
        let Some(key) = self.state_key(key) else {
            return false;
        };

        self.with_inner(|inner| inner.has(&key).unwrap_or(false))
    }
}
