//! Holds the storage writes a host-side collection call makes to the limits
//! `storage_write` and `storage_index_set` enforce on a guest's own writes.

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
        let (set, meta_set) = (index.set, index.meta_set);
        let charge_set = Rc::clone(&charge);
        index.set = Rc::new(move |key: &[u8], value: &[u8]| {
            charge_set(key.len(), value.len()) && set(key, value)
        });
        index.meta_set = Rc::new(move |key: &[u8], value: &[u8]| {
            charge(key.len(), value.len()) && meta_set(key, value)
        });
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
