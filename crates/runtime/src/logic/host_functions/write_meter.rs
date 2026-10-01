//! Holds the storage writes a host-side collection call makes to the limits
//! `storage_write` and the `storage_index_*` writes enforce on a guest's own.

use std::cell::RefCell;
use std::rc::Rc;

use calimero_storage::env::RuntimeEnv;
use calimero_storage::store::Key;

use crate::errors::HostError;
use crate::logic::{charge_write_counters, VMLogic, VMLogicError, VMLogicResult};

/// The execution's write budget while a metered environment is installed, and
/// the first write it refused.
struct Budget {
    writes: u64,
    bytes: u64,
    refused: Option<VMLogicError>,
}

/// Settles a metered environment's writes back into the execution.
pub(super) struct WriteMeter(Rc<RefCell<Budget>>);

/// Wraps `env` so every write it makes is checked and charged like a guest
/// write. A refused write is skipped and reported by [`WriteMeter::settle`].
pub(super) fn metered(env: RuntimeEnv, logic: &VMLogic<'_>) -> (RuntimeEnv, WriteMeter) {
    let budget = Rc::new(RefCell::new(Budget {
        writes: logic.storage_writes,
        bytes: logic.storage_write_bytes,
        refused: None,
    }));
    let limits = logic.limits;
    let (max_key, max_value) = (
        limits.max_storage_key_size.get(),
        limits.max_storage_value_size.get(),
    );
    let (max_writes, max_bytes) = (limits.max_storage_writes, limits.max_storage_write_bytes);

    let charge = {
        let budget = Rc::clone(&budget);
        Rc::new(move |key_len: usize, value_len: usize| -> bool {
            let mut budget = budget.borrow_mut();
            if budget.refused.is_some() {
                return false;
            }
            let (key_len, value_len) = (key_len as u64, value_len as u64);
            let charged = if key_len > max_key {
                Err(HostError::KeyLengthOverflow.into())
            } else if value_len > max_value {
                Err(HostError::ValueLengthOverflow.into())
            } else {
                let Budget { writes, bytes, .. } = &mut *budget;
                charge_write_counters(writes, bytes, max_writes, max_bytes, key_len + value_len)
            };
            charged.map_err(|err| budget.refused = Some(err)).is_ok()
        })
    };

    let write = env.storage_write();
    let writer = {
        let charge = Rc::clone(&charge);
        Rc::new(move |key: Key, value: &[u8]| {
            charge(key.to_bytes().len(), value.len()) && write(key, value)
        })
    };
    let mut metered = RuntimeEnv::new(
        env.storage_read(),
        writer,
        env.storage_remove(),
        env.context_id(),
        env.device_id(),
        env.account_id(),
    );
    if let Some(mut index) = env.index() {
        let (set, meta_set, meta_clear) = (index.set, index.meta_set, index.meta_clear);
        let (charge_set, charge_meta) = (Rc::clone(&charge), Rc::clone(&charge));
        index.set = Rc::new(move |key: &[u8], value: &[u8]| {
            charge_set(key.len(), value.len()) && set(key, value)
        });
        index.meta_set = Rc::new(move |key: &[u8], value: &[u8]| {
            charge_meta(key.len(), value.len()) && meta_set(key, value)
        });
        index.meta_clear = Rc::new(move |key: &[u8]| charge(key.len(), 0) && meta_clear(key));
        metered = metered.with_index(index);
    }
    (metered, WriteMeter(budget))
}

impl WriteMeter {
    /// Commits the charged writes to the execution's budget, or returns the
    /// refusal, which fails the host call and so the whole execution.
    pub(super) fn settle(self, logic: &mut VMLogic<'_>) -> VMLogicResult<()> {
        let mut budget = self.0.borrow_mut();
        if let Some(refused) = budget.refused.take() {
            return Err(refused);
        }
        logic.storage_writes = budget.writes;
        logic.storage_write_bytes = budget.bytes;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use crate::errors::HostError;
    use crate::logic::host_functions::system::build_runtime_env;
    use crate::logic::tests::setup_vm;
    use crate::logic::{Cow, VMContext, VMLimits, VMLogic, VMLogicError, DIGEST_SIZE};
    use crate::store::{InMemoryStorage, Storage};
    use wasmer::Store;

    use super::metered;

    /// `InMemoryStorage` with the ordered index bridged, as a node's storage is.
    #[derive(Default)]
    struct IndexedStorage(InMemoryStorage);

    impl Storage for IndexedStorage {
        fn get(&self, key: &Vec<u8>) -> Option<Vec<u8>> {
            self.0.get(key)
        }
        fn set(&mut self, key: Vec<u8>, value: Vec<u8>) -> Option<Vec<u8>> {
            self.0.set(key, value)
        }
        fn remove(&mut self, key: &Vec<u8>) -> Option<Vec<u8>> {
            self.0.remove(key)
        }
        fn has(&self, key: &Vec<u8>) -> bool {
            self.0.has(key)
        }
        fn supports_index(&self) -> bool {
            true
        }
        fn index_set(&mut self, key: &[u8], value: &[u8]) -> bool {
            self.0.index_set(key, value)
        }
        fn index_del(&mut self, key: &[u8]) -> bool {
            self.0.index_del(key)
        }
        fn index_del_prefix(&mut self, prefix: &[u8]) -> bool {
            self.0.index_del_prefix(prefix)
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
        fn index_meta_set(&mut self, key: &[u8], value: &[u8]) -> bool {
            self.0.index_meta_set(key, value)
        }
        fn index_meta_get(&self, key: &[u8]) -> Option<Vec<u8>> {
            self.0.index_meta_get(key)
        }
        fn index_meta_del(&mut self, key: &[u8]) -> bool {
            self.0.index_meta_del(key)
        }
    }

    /// Runs `write` against the metered ordered index and settles the meter.
    fn settle_index_write(
        limits: &VMLimits,
        write: impl FnOnce(&calimero_storage::env::IndexCallbacks) -> bool,
    ) -> Result<(), VMLogicError> {
        let mut storage = IndexedStorage::default();
        let (mut logic, _store) = setup_vm!(&mut storage, limits, vec![]);
        let env = build_runtime_env(logic.storage, [0; 32], [0; 32], [0; 32]);
        let (env, meter) = metered(env, &logic);
        let index = env
            .index()
            .expect("an index-backed store installs the bridge");
        assert!(!write(&index), "a refused write is skipped");
        meter.settle(&mut logic)
    }

    /// An ordered-index write is held to the key cap `storage_index_set` enforces.
    #[test]
    fn test_index_write_refuses_an_oversize_key() {
        let limits = VMLimits {
            max_storage_key_size: NonZeroU64::new(128).unwrap(),
            ..VMLimits::default()
        };
        let err = settle_index_write(&limits, |index| (index.set)(&[b'k'; 200], b"v"));
        assert!(
            matches!(
                err,
                Err(VMLogicError::HostError(HostError::KeyLengthOverflow))
            ),
            "{err:?}"
        );
    }

    /// Clearing an index marker draws on the write budget, as
    /// `storage_index_meta_clear` does.
    #[test]
    fn test_index_marker_clear_draws_on_the_write_budget() {
        let limits = VMLimits {
            max_storage_writes: 0,
            ..VMLimits::default()
        };
        let err = settle_index_write(&limits, |index| (index.meta_clear)(b"marker"));
        assert!(
            matches!(
                err,
                Err(VMLogicError::HostError(
                    HostError::StorageWriteCountExceeded { max: 0 }
                ))
            ),
            "{err:?}"
        );
    }
}
