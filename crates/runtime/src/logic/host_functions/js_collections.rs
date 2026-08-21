use crate::panic_payload::panic_payload_to_string;
use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicResult},
};
use calimero_storage::{
    address::Id,
    entities::{ChildInfo, Data, Metadata},
    env::{time_now, with_runtime_env, RuntimeEnv},
    index::Index,
    interface::{Interface, StorageError},
    js::{
        JsAuthoredMap, JsAuthoredVector, JsCounter, JsFrozenStorage, JsLwwRegister, JsPnCounter,
        JsRga, JsSharedStorage, JsSortedMap, JsSortedSet, JsUnorderedMap, JsUnorderedSet,
        JsUserStorage, JsVector,
    },
    store::MainStorage,
};
use std::{
    convert::TryFrom,
    fmt::Display,
    panic::{self, AssertUnwindSafe},
};
use tracing::{debug, warn};

use super::system::build_runtime_env;

const COLLECTION_ID_LEN: usize = 32;
/// Byte length of an Ed25519 public key, the unit of a serialized writer set.
const PUBLIC_KEY_LEN: usize = 32;

/// Generate the `*_new_with_id` host function for one JS-facing collection type.
///
/// Thirteen copies of this existed, byte-identical apart from the type and which
/// saver they called. Each is the same four steps — read a caller-supplied id, mint
/// the collection at it, persist it, hand the id back — and the only interesting
/// thing about them is what they must do when the guest panics mid-construction.
///
/// **Why the `catch_unwind`.** Minting and persisting runs guest-reachable storage
/// code, and a panic crossing the host boundary aborts the whole call with no
/// diagnostic the app can see. Catching it here turns it into an error message in
/// the destination register, which is the same channel every other failure in this
/// file uses. Thirteen copies of that reasoning is thirteen chances for one of them
/// to lose it.
/// Generate the `*_new` host function for one JS-facing collection type.
///
/// The sibling of [`js_new_with_id_op`], for the case where the host mints the id
/// instead of the caller supplying it. Thirteen copies, one shape; the same
/// `catch_unwind` reasoning applies, and applies for the same reason.
macro_rules! js_new_op {
    ($op:ident, $ty:ty, save: $save:ident $(,)?) => {
        fn $op(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
            let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<$ty, String> {
                let mut value = <$ty>::new();
                $save(&mut value)?;
                Ok(value)
            }));

            match outcome {
                Ok(Ok(value)) => {
                    self.write_register_bytes(dest_register_id, value.id().as_bytes())?;
                    Ok(0)
                }
                Ok(Err(err)) => self.write_error_message(dest_register_id, err),
                Err(payload) => self.write_error_message(
                    dest_register_id,
                    panic_payload_to_string(payload.as_ref(), "unknown panic"),
                ),
            }
        }
    };
}

macro_rules! js_new_with_id_op {
    ($op:ident, $ty:ty, save: $save:ident $(,)?) => {
        fn $op(&mut self, id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
            let id = match self.read_map_id(id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<$ty, String> {
                let mut value = <$ty>::new_with_id(id);
                $save(&mut value)?;
                Ok(value)
            }));

            match outcome {
                Ok(Ok(value)) => {
                    self.write_register_bytes(dest_register_id, value.id().as_bytes())?;
                    Ok(0)
                }
                Ok(Err(err)) => self.write_error_message(dest_register_id, err),
                Err(payload) => self.write_error_message(
                    dest_register_id,
                    panic_payload_to_string(payload.as_ref(), "unknown panic"),
                ),
            }
        }
    };
}

// The plain and authored halves of these five reads are the same code; only the
// loader differs. Generating both from one body is what stops them drifting apart
// unnoticed — before this they sat ~1500 lines apart in the same impl.
//
// `insert`, `push` and `remove` are deliberately NOT generated. Their halves only
// look alike: authored `insert` rejects an already-present key where plain upserts
// and returns the previous value, authored `push` returns the new index through an
// extra register argument, and authored `remove` is owner-only. Those differences
// are intentional and covered by tests in `calimero-storage`; folding them in would
// assert an equivalence the design rejects.
/// `map_get` for a plain and an authored collection: the same host-function body,
/// differing only in which loader resolves the instance id.
macro_rules! js_map_get {
    ($name:ident, $loader:ident) => {
        fn $name(
            &mut self,
            map_id_ptr: u64,
            key_ptr: u64,
            dest_register_id: u64,
        ) -> VMLogicResult<i32> {
            let map_id = match self.read_map_id(map_id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let key = self.read_buffer(key_ptr)?;

            let map = match $loader(map_id) {
                Ok(map) => map,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            match map.get(&key) {
                Ok(Some(value)) => {
                    self.write_register_bytes(dest_register_id, &value)?;
                    Ok(1)
                }
                Ok(None) => {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
                Err(err) => self.write_error_message(dest_register_id, err),
            }
        }
    };
}
/// `vector_get` for a plain and an authored collection: the same host-function body,
/// differing only in which loader resolves the instance id.
macro_rules! js_vector_get {
    ($name:ident, $loader:ident) => {
        fn $name(
            &mut self,
            vector_id_ptr: u64,
            index: u64,
            dest_register_id: u64,
        ) -> VMLogicResult<i32> {
            let vector_id = match self.read_map_id(vector_id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let idx = match usize::try_from(index) {
                Ok(value) => value,
                Err(_) => {
                    return self.write_error_message(
                        dest_register_id,
                        format!("index {index} does not fit into usize"),
                    )
                }
            };

            let vector = match $loader(vector_id) {
                Ok(vector) => vector,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            match vector.get(idx) {
                Ok(Some(value)) => {
                    self.write_register_bytes(dest_register_id, &value)?;
                    Ok(1)
                }
                Ok(None) => {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
                Err(err) => self.write_error_message(dest_register_id, err),
            }
        }
    };
}
/// `map_contains` for a plain and an authored collection: the same host-function body,
/// differing only in which loader resolves the instance id.
macro_rules! js_map_contains {
    ($name:ident, $loader:ident) => {
        fn $name(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
            let map_id = match self.read_map_id(map_id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(0, message),
            };

            let key = self.read_buffer(key_ptr)?;

            let map = match $loader(map_id) {
                Ok(map) => map,
                Err(message) => return self.write_error_message(0, message),
            };

            match map.contains(&key) {
                Ok(result) => Ok(i32::from(result)),
                Err(err) => self.write_error_message(0, err),
            }
        }
    };
}
/// `map_iter` for a plain and an authored collection: the same host-function body,
/// differing only in which loader resolves the instance id.
macro_rules! js_map_iter {
    ($name:ident, $loader:ident) => {
        fn $name(&mut self, map_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
            let map_id = match self.read_map_id(map_id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let map = match $loader(map_id) {
                Ok(map) => map,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let entries = match map.entries() {
                Ok(entries) => entries,
                Err(err) => return self.write_error_message(dest_register_id, err),
            };

            let count = u32::try_from(entries.len()).map_err(|_| HostError::IntegerOverflow)?;

            let mut total_len: usize = 4;
            for (key, value) in &entries {
                let key_len = key.len();
                let value_len = value.len();
                u32::try_from(key_len).map_err(|_| HostError::IntegerOverflow)?;
                u32::try_from(value_len).map_err(|_| HostError::IntegerOverflow)?;
                total_len = total_len
                    .checked_add(4)
                    .and_then(|acc| acc.checked_add(key_len))
                    .and_then(|acc| acc.checked_add(4))
                    .and_then(|acc| acc.checked_add(value_len))
                    .ok_or(HostError::IntegerOverflow)?;
            }

            let mut buffer = Vec::with_capacity(total_len);
            buffer.extend_from_slice(&count.to_le_bytes());
            for (key, value) in entries {
                let key_len = u32::try_from(key.len()).map_err(|_| HostError::IntegerOverflow)?;
                let value_len =
                    u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
                buffer.extend_from_slice(&key_len.to_le_bytes());
                buffer.extend_from_slice(&key);
                buffer.extend_from_slice(&value_len.to_le_bytes());
                buffer.extend_from_slice(&value);
            }

            self.write_register_bytes(dest_register_id, &buffer)?;
            Ok(1)
        }
    };
}
/// `vector_len` for a plain and an authored collection: the same host-function body,
/// differing only in which loader resolves the instance id.
macro_rules! js_vector_len {
    ($name:ident, $loader:ident) => {
        fn $name(&mut self, vector_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
            let vector_id = match self.read_map_id(vector_id_ptr)? {
                Ok(id) => id,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            let vector = match $loader(vector_id) {
                Ok(vector) => vector,
                Err(message) => return self.write_error_message(dest_register_id, message),
            };

            match vector.len() {
                Ok(len) => {
                    let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                    self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                    Ok(1)
                }
                Err(err) => self.write_error_message(dest_register_id, err),
            }
        }
    };
}

impl VMHostFunctions<'_> {
    fn make_runtime_env(&mut self) -> VMLogicResult<RuntimeEnv> {
        self.with_logic_mut(|logic| {
            Ok(build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
                logic.context.account_id,
            ))
        })
    }

    fn invoke_with_storage_env<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> VMLogicResult<T>,
    ) -> VMLogicResult<T> {
        let env = self.make_runtime_env()?;
        with_runtime_env(env, || f(self))
    }

    /// Creates a new CRDT map and returns its identifier.
    pub fn js_crdt_map_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_new(dest_register_id))
    }

    /// Creates a new CRDT map at a caller-supplied deterministic id.
    pub fn js_crdt_map_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_new_with_id(id_ptr, dest_register_id))
    }

    /// Retrieves a value from the CRDT map.
    pub fn js_crdt_map_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_map_get(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    /// Inserts or replaces a value in the CRDT map.
    pub fn js_crdt_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_map_insert(map_id_ptr, key_ptr, value_ptr, dest_register_id)
        })
    }

    /// Removes a value from the CRDT map.
    pub fn js_crdt_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_map_remove(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    /// Checks whether a key exists in the CRDT map.
    pub fn js_crdt_map_contains(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_contains(map_id_ptr, key_ptr))
    }

    pub fn js_crdt_map_iter(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_map_iter(map_id_ptr, dest_register_id))
    }

    /// Creates a new vector collection.
    pub fn js_crdt_vector_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_new(dest_register_id))
    }

    /// Creates a new vector collection at a caller-supplied deterministic id.
    pub fn js_crdt_vector_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_vector_len(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_len(vector_id_ptr, dest_register_id))
    }

    pub fn js_crdt_vector_push(
        &mut self,
        vector_id_ptr: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_push(vector_id_ptr, value_ptr))
    }

    pub fn js_crdt_vector_get(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_vector_get(vector_id_ptr, index, dest_register_id)
        })
    }

    pub fn js_crdt_vector_pop(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_vector_pop(vector_id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_new(dest_register_id))
    }

    /// Creates a new set collection at a caller-supplied deterministic id.
    pub fn js_crdt_set_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_insert(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_insert(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_set_contains(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_contains(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_set_remove(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_remove(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_set_len(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_len(set_id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_iter(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_iter(set_id_ptr, dest_register_id))
    }

    pub fn js_crdt_set_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_set_clear(set_id_ptr))
    }

    pub fn js_crdt_lww_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_lww_new(dest_register_id))
    }

    /// Creates a new LWW register at a caller-supplied deterministic id.
    pub fn js_crdt_lww_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_lww_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_lww_set(
        &mut self,
        register_id_ptr: u64,
        value_ptr: u64,
        has_value: u32,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_lww_set(register_id_ptr, value_ptr, has_value)
        })
    }

    pub fn js_crdt_lww_get(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_lww_get(register_id_ptr, dest_register_id))
    }

    pub fn js_crdt_lww_timestamp(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_lww_timestamp(register_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_counter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_counter_new(dest_register_id))
    }

    /// Creates a new counter at a caller-supplied deterministic id.
    pub fn js_crdt_counter_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_counter_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_counter_increment(counter_id_ptr))
    }

    pub fn js_crdt_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_counter_value(counter_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_counter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_counter_get_executor_count(
                counter_id_ptr,
                executor_ptr,
                has_executor,
                dest_register_id,
            )
        })
    }

    /// Creates a new UserStorage and returns its identifier.
    pub fn js_user_storage_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_new(dest_register_id))
    }

    /// Creates a new UserStorage at a caller-supplied deterministic id.
    pub fn js_user_storage_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_new_with_id(id_ptr, dest_register_id))
    }

    /// Inserts or replaces a value in UserStorage for the current executor.
    pub fn js_user_storage_insert(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_insert(storage_id_ptr, value_ptr, dest_register_id)
        })
    }

    /// Retrieves a value from UserStorage for the current executor.
    pub fn js_user_storage_get(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_get(storage_id_ptr, dest_register_id))
    }

    /// Retrieves a value from UserStorage for a specific user.
    pub fn js_user_storage_get_for_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_get_for_user(storage_id_ptr, user_key_ptr, dest_register_id)
        })
    }

    /// Removes a value from UserStorage for the current executor.
    pub fn js_user_storage_remove(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_remove(storage_id_ptr, dest_register_id)
        })
    }

    /// Checks whether data exists for the current executor in UserStorage.
    pub fn js_user_storage_contains(&mut self, storage_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.user_storage_contains(storage_id_ptr))
    }

    /// Checks whether data exists for a specific user in UserStorage.
    pub fn js_user_storage_contains_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.user_storage_contains_user(storage_id_ptr, user_key_ptr)
        })
    }

    /// Creates a new FrozenStorage and returns its identifier.
    pub fn js_frozen_storage_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.frozen_storage_new(dest_register_id))
    }

    /// Creates a new FrozenStorage at a caller-supplied deterministic id.
    pub fn js_frozen_storage_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.frozen_storage_new_with_id(id_ptr, dest_register_id)
        })
    }

    /// Inserts a value into FrozenStorage and returns its hash.
    pub fn js_frozen_storage_add(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.frozen_storage_add(storage_id_ptr, value_ptr, dest_register_id)
        })
    }

    /// Retrieves a value from FrozenStorage by hash.
    pub fn js_frozen_storage_get(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.frozen_storage_get(storage_id_ptr, hash_ptr, dest_register_id)
        })
    }

    /// Checks whether a hash exists in FrozenStorage.
    pub fn js_frozen_storage_contains(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.frozen_storage_contains(storage_id_ptr, hash_ptr))
    }

    /// Creates a new PN-counter and returns its identifier.
    pub fn js_crdt_pncounter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_pncounter_new(dest_register_id))
    }

    /// Creates a new PN-counter at a caller-supplied deterministic id.
    pub fn js_crdt_pncounter_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pncounter_new_with_id(id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_pncounter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_pncounter_increment(counter_id_ptr))
    }

    pub fn js_crdt_pncounter_decrement(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_pncounter_decrement(counter_id_ptr))
    }

    pub fn js_crdt_pncounter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pncounter_value(counter_id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_pncounter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_pncounter_get_executor_count(
                counter_id_ptr,
                executor_ptr,
                has_executor,
                dest_register_id,
            )
        })
    }

    /// Creates a new RGA (collaborative text sequence) and returns its identifier.
    pub fn js_crdt_rga_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_new(dest_register_id))
    }

    /// Creates a new RGA at a caller-supplied deterministic id.
    pub fn js_crdt_rga_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_new_with_id(id_ptr, dest_register_id))
    }

    pub fn js_crdt_rga_insert(
        &mut self,
        rga_id_ptr: u64,
        index: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_insert(rga_id_ptr, index, value_ptr))
    }

    pub fn js_crdt_rga_delete(&mut self, rga_id_ptr: u64, index: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_delete(rga_id_ptr, index))
    }

    pub fn js_crdt_rga_get_text(
        &mut self,
        rga_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_get_text(rga_id_ptr, dest_register_id))
    }

    pub fn js_crdt_rga_len(
        &mut self,
        rga_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_rga_len(rga_id_ptr, dest_register_id))
    }

    /// Creates a new ordered CRDT map and returns its identifier.
    pub fn js_crdt_sortedmap_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedmap_new(dest_register_id))
    }

    /// Creates a new ordered CRDT map at a caller-supplied deterministic id.
    pub fn js_crdt_sortedmap_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_sortedmap_new_with_id(id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_sortedmap_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_sortedmap_get(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_sortedmap_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_sortedmap_insert(map_id_ptr, key_ptr, value_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_sortedmap_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_sortedmap_remove(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_sortedmap_contains(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedmap_contains(map_id_ptr, key_ptr))
    }

    pub fn js_crdt_sortedmap_iter(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedmap_iter(map_id_ptr, dest_register_id))
    }

    /// Creates a new ordered CRDT set and returns its identifier.
    pub fn js_crdt_sortedset_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_new(dest_register_id))
    }

    /// Creates a new ordered CRDT set at a caller-supplied deterministic id.
    pub fn js_crdt_sortedset_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_sortedset_new_with_id(id_ptr, dest_register_id)
        })
    }

    pub fn js_crdt_sortedset_insert(
        &mut self,
        set_id_ptr: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_insert(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_sortedset_contains(
        &mut self,
        set_id_ptr: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_contains(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_sortedset_remove(
        &mut self,
        set_id_ptr: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_remove(set_id_ptr, value_ptr))
    }

    pub fn js_crdt_sortedset_len(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_len(set_id_ptr, dest_register_id))
    }

    pub fn js_crdt_sortedset_iter(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_iter(set_id_ptr, dest_register_id))
    }

    pub fn js_crdt_sortedset_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_sortedset_clear(set_id_ptr))
    }

    /// Creates a new attributed CRDT map and returns its identifier.
    pub fn js_crdt_authored_map_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_authored_map_new(dest_register_id))
    }

    /// Creates a new attributed CRDT map at a caller-supplied deterministic id.
    pub fn js_crdt_authored_map_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_new_with_id(id_ptr, dest_register_id)
        })
    }

    /// Inserts a new key/value pair, stamping the caller as the entry owner.
    pub fn js_crdt_authored_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_insert(map_id_ptr, key_ptr, value_ptr, dest_register_id)
        })
    }

    /// Updates the value at a key. Only the entry owner may call this.
    pub fn js_crdt_authored_map_update(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_update(map_id_ptr, key_ptr, value_ptr, dest_register_id)
        })
    }

    /// Removes a key. Only the entry owner may call this.
    pub fn js_crdt_authored_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_remove(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    /// Retrieves the value at a key.
    pub fn js_crdt_authored_map_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_get(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    /// Checks whether a key exists in the attributed map.
    pub fn js_crdt_authored_map_contains(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_authored_map_contains(map_id_ptr, key_ptr))
    }

    /// Writes the 32-byte owner public key of a key to the register.
    pub fn js_crdt_authored_map_owner_of(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_owner_of(map_id_ptr, key_ptr, dest_register_id)
        })
    }

    /// Returns whether the current executor owns a key.
    pub fn js_crdt_authored_map_owned_by_me(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_authored_map_owned_by_me(map_id_ptr, key_ptr))
    }

    /// Iterates all entries in the attributed map.
    pub fn js_crdt_authored_map_iter(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_iter(map_id_ptr, dest_register_id)
        })
    }

    /// Returns the number of entries in the attributed map.
    pub fn js_crdt_authored_map_len(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_map_len(map_id_ptr, dest_register_id)
        })
    }

    /// Creates a new attributed CRDT vector and returns its identifier.
    pub fn js_crdt_authored_vector_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_authored_vector_new(dest_register_id))
    }

    /// Creates a new attributed CRDT vector at a caller-supplied deterministic id.
    pub fn js_crdt_authored_vector_new_with_id(
        &mut self,
        id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_new_with_id(id_ptr, dest_register_id)
        })
    }

    /// Pushes a new value at the tail, stamping the caller as the slot owner.
    /// Writes the new index (u64 LE) to the register.
    pub fn js_crdt_authored_vector_push(
        &mut self,
        vector_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_push(vector_id_ptr, value_ptr, dest_register_id)
        })
    }

    /// Updates the value at a slot. Only the slot owner may call this.
    pub fn js_crdt_authored_vector_update(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_update(vector_id_ptr, index, value_ptr, dest_register_id)
        })
    }

    /// Tombstones a slot. Only the slot owner may call this.
    pub fn js_crdt_authored_vector_tombstone(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_tombstone(vector_id_ptr, index, dest_register_id)
        })
    }

    /// Retrieves the value at a slot.
    pub fn js_crdt_authored_vector_get(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_get(vector_id_ptr, index, dest_register_id)
        })
    }

    /// Writes the 32-byte owner public key of a slot to the register.
    pub fn js_crdt_authored_vector_owner_of(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_owner_of(vector_id_ptr, index, dest_register_id)
        })
    }

    /// Returns whether the current executor owns a slot.
    pub fn js_crdt_authored_vector_owned_by_me(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_owned_by_me(vector_id_ptr, index)
        })
    }

    /// Iterates all values in the attributed vector (insertion order).
    pub fn js_crdt_authored_vector_iter(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_iter(vector_id_ptr, dest_register_id)
        })
    }

    /// Returns the number of entries in the attributed vector.
    pub fn js_crdt_authored_vector_len(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_authored_vector_len(vector_id_ptr, dest_register_id)
        })
    }

    /// Creates a new group-writable shared byte cell with the given writer set
    /// (a buffer of concatenated 32-byte public keys) and returns its identifier.
    pub fn js_crdt_shared_new(
        &mut self,
        writers_ptr: u64,
        frozen: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_shared_new(writers_ptr, frozen, dest_register_id)
        })
    }

    /// Creates a new shared byte cell at a caller-supplied deterministic id.
    pub fn js_crdt_shared_new_with_id(
        &mut self,
        id_ptr: u64,
        writers_ptr: u64,
        frozen: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_shared_new_with_id(id_ptr, writers_ptr, frozen, dest_register_id)
        })
    }

    /// Replaces the value. Writer-gated: a non-writer is rejected with
    /// `ActionNotAllowed` (written to register 0).
    pub fn js_crdt_shared_set(&mut self, cell_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_shared_set(cell_id_ptr, value_ptr))
    }

    /// Reads the current value into the register (status 1), or clears it if no
    /// value has been written (status 0).
    pub fn js_crdt_shared_get(
        &mut self,
        cell_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_shared_get(cell_id_ptr, dest_register_id))
    }

    /// Writes the current writer set as concatenated 32-byte public keys to the
    /// register (the JS side decodes `len / 32` keys).
    pub fn js_crdt_shared_writers(
        &mut self,
        cell_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_shared_writers(cell_id_ptr, dest_register_id))
    }

    /// Returns whether the current executor is in the writer set (1) or not (0).
    pub fn js_crdt_shared_writable_by_me(&mut self, cell_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_shared_writable_by_me(cell_id_ptr))
    }

    /// Returns whether the writer set is frozen (1) or not (0).
    pub fn js_crdt_shared_is_frozen(&mut self, cell_id_ptr: u64) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_shared_is_frozen(cell_id_ptr))
    }

    /// Rotates the writer set to the given keys (concatenated 32-byte public
    /// keys). Writer-gated: a non-writer (or a frozen/empty rotation) is rejected
    /// with `ActionNotAllowed` (written to register 0).
    pub fn js_crdt_shared_rotate_writers(
        &mut self,
        cell_id_ptr: u64,
        writers_ptr: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| {
            host.crdt_shared_rotate_writers(cell_id_ptr, writers_ptr)
        })
    }

    pub fn js_crdt_delete_collection(
        &mut self,
        id_ptr: u64,
        register_id: u64,
    ) -> VMLogicResult<i32> {
        self.invoke_with_storage_env(|host| host.crdt_delete_collection(id_ptr, register_id))
    }

    js_map_get!(crdt_map_get, load_js_map_instance);
    js_map_get!(crdt_authored_map_get, load_js_authored_map_instance);

    fn crdt_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;
        let value = self.read_buffer(value_ptr)?;

        let mut map = match load_js_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match map.insert(&key, &value) {
            Ok(previous) => {
                if let Err(message) = save_js_instance(&mut map) {
                    return self.write_error_message(dest_register_id, message);
                }

                if let Some(prev) = previous {
                    self.write_register_bytes(dest_register_id, &prev)?;
                    Ok(1)
                } else {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let mut map = match load_js_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match map.remove(&key) {
            Ok(Some(previous)) => {
                if let Err(message) = save_js_instance(&mut map) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &previous)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    js_map_contains!(crdt_map_contains, load_js_map_instance);
    js_map_contains!(crdt_authored_map_contains, load_js_authored_map_instance);

    js_map_iter!(crdt_map_iter, load_js_map_instance);
    js_map_iter!(crdt_authored_map_iter, load_js_authored_map_instance);

    js_vector_len!(crdt_vector_len, load_js_vector_instance);
    js_vector_len!(crdt_authored_vector_len, load_js_authored_vector_instance);

    fn crdt_vector_push(&mut self, vector_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut vector = match load_js_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(0, message),
        };

        match vector.push(&value) {
            Ok(()) => match save_js_instance(&mut vector) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    js_vector_get!(crdt_vector_get, load_js_vector_instance);
    js_vector_get!(crdt_authored_vector_get, load_js_authored_vector_instance);

    fn crdt_vector_pop(&mut self, vector_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let mut vector = match load_js_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match vector.pop() {
            Ok(Some(value)) => {
                if let Err(message) = save_js_instance(&mut vector) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_set_insert(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.insert(&value) {
            Ok(inserted) => {
                if !inserted {
                    return Ok(0);
                }
                if let Err(message) = save_js_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_set_contains(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.contains(&value) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_set_remove(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.remove(&value) {
            Ok(removed) => {
                if !removed {
                    return Ok(0);
                }
                if let Err(message) = save_js_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_set_len(&mut self, set_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match set.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_set_iter(&mut self, set_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let values = match set.values() {
            Ok(values) => values,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };

        let count = u32::try_from(values.len()).map_err(|_| HostError::IntegerOverflow)?;

        let mut total_len: usize = 4;
        for value in &values {
            let value_len = value.len();
            u32::try_from(value_len).map_err(|_| HostError::IntegerOverflow)?;
            total_len = total_len
                .checked_add(4)
                .and_then(|acc| acc.checked_add(value_len))
                .ok_or(HostError::IntegerOverflow)?;
        }

        let mut buffer = Vec::with_capacity(total_len);
        buffer.extend_from_slice(&count.to_le_bytes());
        for value in values {
            let value_len = u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            buffer.extend_from_slice(&value_len.to_le_bytes());
            buffer.extend_from_slice(&value);
        }

        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_set_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        let len_before = match set.len() {
            Ok(len) => len,
            Err(err) => return self.write_error_message(0, err),
        };

        match set.clear() {
            Ok(()) => {
                if len_before == 0 {
                    return Ok(0);
                }
                if let Err(message) = save_js_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_lww_set(
        &mut self,
        register_id_ptr: u64,
        value_ptr: u64,
        has_value: u32,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = if has_value != 0 {
            Some(self.read_buffer(value_ptr)?)
        } else {
            None
        };

        let mut register = match load_js_lww_instance(register_id) {
            Ok(register) => register,
            Err(message) => return self.write_error_message(0, message),
        };

        let previous_value = register.get();
        let values_equal = previous_value.as_deref() == value.as_deref();
        register.set(value.as_deref());

        match save_js_lww_instance(&mut register) {
            Ok(()) => Ok(i32::from(!values_equal)),
            Err(message) => self.write_error_message(0, message),
        }
    }

    fn crdt_lww_get(&mut self, register_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let register = match load_js_lww_instance(register_id) {
            Ok(register) => register,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match register.get() {
            Some(value) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            None => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
        }
    }

    fn crdt_lww_timestamp(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let register = match load_js_lww_instance(register_id) {
            Ok(register) => register,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        if register.get().is_none() {
            self.clear_register(dest_register_id)?;
            return Ok(0);
        }

        let timestamp = register.timestamp();
        let time_le = timestamp.get_time().as_u64().to_le_bytes();
        let node_id: u128 = (*timestamp.get_id()).into();
        let mut buffer = [0u8; 24];
        buffer[..8].copy_from_slice(&time_le);
        buffer[8..].copy_from_slice(&node_id.to_le_bytes());

        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut counter = match load_js_counter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(0, message),
        };

        match counter.increment() {
            Ok(()) => match save_js_counter_instance(&mut counter) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_counter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let counter = match load_js_counter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match counter.value() {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_counter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let executor_bytes: [u8; 32] = if has_executor != 0 {
            let bytes = self.read_buffer(executor_ptr)?;
            match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(array) => array,
                Err(_) => {
                    return self.write_error_message(
                        dest_register_id,
                        "executor id must be exactly 32 bytes",
                    )
                }
            }
        } else {
            self.borrow_logic().context.executor_public_key
        };

        let counter = match load_js_counter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match counter.get_executor_count(&executor_bytes) {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_insert(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut storage = match load_js_user_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match storage.insert(&value) {
            Ok(previous) => {
                if let Err(message) = save_js_instance(&mut storage) {
                    return self.write_error_message(dest_register_id, message);
                }

                if let Some(prev) = previous {
                    self.write_register_bytes(dest_register_id, &prev)?;
                    Ok(1)
                } else {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_get(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let storage = match load_js_user_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match storage.get() {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_get_for_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let user_key_bytes = self.read_buffer(user_key_ptr)?;
        let user_key: [u8; 32] = match <[u8; 32]>::try_from(user_key_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => {
                return self
                    .write_error_message(dest_register_id, "user key must be exactly 32 bytes")
            }
        };

        let storage = match load_js_user_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match storage.get_for_user(&user_key) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_remove(
        &mut self,
        storage_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let mut storage = match load_js_user_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match storage.remove() {
            Ok(Some(previous)) => {
                if let Err(message) = save_js_instance(&mut storage) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &previous)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn user_storage_contains(&mut self, storage_id_ptr: u64) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let storage = match load_js_user_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(0, message),
        };

        match storage.contains_current_user() {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn user_storage_contains_user(
        &mut self,
        storage_id_ptr: u64,
        user_key_ptr: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let user_key_bytes = self.read_buffer(user_key_ptr)?;
        let user_key: [u8; 32] = match <[u8; 32]>::try_from(user_key_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => return self.write_error_message(0, "user key must be exactly 32 bytes"),
        };

        let storage = match load_js_user_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(0, message),
        };

        match storage.contains_user(&user_key) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn frozen_storage_add(
        &mut self,
        storage_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut storage = match load_js_frozen_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match storage.insert(&value) {
            Ok(hash) => {
                if let Err(message) = save_js_instance(&mut storage) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &hash)?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn frozen_storage_get(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let hash_bytes = self.read_buffer(hash_ptr)?;
        let hash: [u8; 32] = match <[u8; 32]>::try_from(hash_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => {
                return self.write_error_message(dest_register_id, "hash must be exactly 32 bytes")
            }
        };

        let storage = match load_js_frozen_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match storage.get(&hash) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn frozen_storage_contains(
        &mut self,
        storage_id_ptr: u64,
        hash_ptr: u64,
    ) -> VMLogicResult<i32> {
        let storage_id = match self.read_map_id(storage_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let hash_bytes = self.read_buffer(hash_ptr)?;
        let hash: [u8; 32] = match <[u8; 32]>::try_from(hash_bytes.as_slice()) {
            Ok(array) => array,
            Err(_) => return self.write_error_message(0, "hash must be exactly 32 bytes"),
        };

        let storage = match load_js_frozen_storage_instance(storage_id) {
            Ok(storage) => storage,
            Err(message) => return self.write_error_message(0, message),
        };

        match storage.contains(&hash) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_pncounter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut counter = match load_js_pncounter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(0, message),
        };

        match counter.increment() {
            Ok(()) => match save_js_instance(&mut counter) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_pncounter_decrement(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut counter = match load_js_pncounter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(0, message),
        };

        match counter.decrement() {
            Ok(()) => match save_js_instance(&mut counter) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_pncounter_value(
        &mut self,
        counter_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let counter = match load_js_pncounter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match counter.value() {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_pncounter_get_executor_count(
        &mut self,
        counter_id_ptr: u64,
        executor_ptr: u64,
        has_executor: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let counter_id = match self.read_map_id(counter_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let executor_bytes: [u8; 32] = if has_executor != 0 {
            let bytes = self.read_buffer(executor_ptr)?;
            match <[u8; 32]>::try_from(bytes.as_slice()) {
                Ok(array) => array,
                Err(_) => {
                    return self.write_error_message(
                        dest_register_id,
                        "executor id must be exactly 32 bytes",
                    )
                }
            }
        } else {
            self.borrow_logic().context.executor_public_key
        };

        let counter = match load_js_pncounter_instance(counter_id) {
            Ok(counter) => counter,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match counter.get_executor_count(&executor_bytes) {
            Ok(value) => {
                self.write_register_bytes(dest_register_id, &value.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_rga_insert(
        &mut self,
        rga_id_ptr: u64,
        index: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self
                    .write_error_message(0, format!("index {index} does not fit into usize"))
            }
        };

        let value = self.read_buffer(value_ptr)?;

        let mut rga = match load_js_rga_instance(rga_id) {
            Ok(rga) => rga,
            Err(message) => return self.write_error_message(0, message),
        };

        match rga.insert(idx, &value) {
            Ok(()) => match save_js_instance(&mut rga) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err.to_string()),
        }
    }

    fn crdt_rga_delete(&mut self, rga_id_ptr: u64, index: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self
                    .write_error_message(0, format!("index {index} does not fit into usize"))
            }
        };

        let mut rga = match load_js_rga_instance(rga_id) {
            Ok(rga) => rga,
            Err(message) => return self.write_error_message(0, message),
        };

        match rga.delete(idx) {
            Ok(()) => match save_js_instance(&mut rga) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err.to_string()),
        }
    }

    fn crdt_rga_get_text(&mut self, rga_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let rga = match load_js_rga_instance(rga_id) {
            Ok(rga) => rga,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match rga.get_text() {
            Ok(text) => {
                self.write_register_bytes(dest_register_id, &text)?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err.to_string()),
        }
    }

    fn crdt_rga_len(&mut self, rga_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let rga_id = match self.read_map_id(rga_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let rga = match load_js_rga_instance(rga_id) {
            Ok(rga) => rga,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match rga.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err.to_string()),
        }
    }

    fn crdt_sortedmap_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let map = match load_js_sortedmap_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match map.get(&key) {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_sortedmap_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;
        let value = self.read_buffer(value_ptr)?;

        let mut map = match load_js_sortedmap_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match map.insert(&key, &value) {
            Ok(previous) => {
                if let Err(message) = save_js_instance(&mut map) {
                    return self.write_error_message(dest_register_id, message);
                }

                if let Some(prev) = previous {
                    self.write_register_bytes(dest_register_id, &prev)?;
                    Ok(1)
                } else {
                    self.clear_register(dest_register_id)?;
                    Ok(0)
                }
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_sortedmap_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let mut map = match load_js_sortedmap_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match map.remove(&key) {
            Ok(Some(previous)) => {
                if let Err(message) = save_js_instance(&mut map) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &previous)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_sortedmap_contains(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let map = match load_js_sortedmap_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(0, message),
        };

        match map.contains(&key) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_sortedmap_iter(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let map = match load_js_sortedmap_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let entries = match map.entries() {
            Ok(entries) => entries,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };

        let count = u32::try_from(entries.len()).map_err(|_| HostError::IntegerOverflow)?;

        let mut total_len: usize = 4;
        for (key, value) in &entries {
            let key_len = key.len();
            let value_len = value.len();
            u32::try_from(key_len).map_err(|_| HostError::IntegerOverflow)?;
            u32::try_from(value_len).map_err(|_| HostError::IntegerOverflow)?;
            total_len = total_len
                .checked_add(4)
                .and_then(|acc| acc.checked_add(key_len))
                .and_then(|acc| acc.checked_add(4))
                .and_then(|acc| acc.checked_add(value_len))
                .ok_or(HostError::IntegerOverflow)?;
        }

        let mut buffer = Vec::with_capacity(total_len);
        buffer.extend_from_slice(&count.to_le_bytes());
        for (key, value) in entries {
            let key_len = u32::try_from(key.len()).map_err(|_| HostError::IntegerOverflow)?;
            let value_len = u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            buffer.extend_from_slice(&key_len.to_le_bytes());
            buffer.extend_from_slice(&key);
            buffer.extend_from_slice(&value_len.to_le_bytes());
            buffer.extend_from_slice(&value);
        }

        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_sortedset_insert(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut set = match load_js_sortedset_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.insert(&value) {
            Ok(inserted) => {
                if !inserted {
                    return Ok(0);
                }
                if let Err(message) = save_js_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_sortedset_contains(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let set = match load_js_sortedset_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.contains(&value) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_sortedset_remove(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut set = match load_js_sortedset_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.remove(&value) {
            Ok(removed) => {
                if !removed {
                    return Ok(0);
                }
                if let Err(message) = save_js_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_sortedset_len(&mut self, set_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let set = match load_js_sortedset_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match set.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_sortedset_iter(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let set = match load_js_sortedset_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let values = match set.values() {
            Ok(values) => values,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };

        let count = u32::try_from(values.len()).map_err(|_| HostError::IntegerOverflow)?;

        let mut total_len: usize = 4;
        for value in &values {
            let value_len = value.len();
            u32::try_from(value_len).map_err(|_| HostError::IntegerOverflow)?;
            total_len = total_len
                .checked_add(4)
                .and_then(|acc| acc.checked_add(value_len))
                .ok_or(HostError::IntegerOverflow)?;
        }

        let mut buffer = Vec::with_capacity(total_len);
        buffer.extend_from_slice(&count.to_le_bytes());
        for value in values {
            let value_len = u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            buffer.extend_from_slice(&value_len.to_le_bytes());
            buffer.extend_from_slice(&value);
        }

        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_sortedset_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut set = match load_js_sortedset_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        let len_before = match set.len() {
            Ok(len) => len,
            Err(err) => return self.write_error_message(0, err),
        };

        match set.clear() {
            Ok(()) => {
                if len_before == 0 {
                    return Ok(0);
                }
                if let Err(message) = save_js_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_authored_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;
        let value = self.read_buffer(value_ptr)?;

        let mut map = match load_js_authored_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // `insert` stamps the current executor (installed in the runtime env) as
        // the entry owner and rejects an already-present key.
        match map.insert(&key, &value) {
            Ok(()) => {
                if let Err(message) = save_js_instance(&mut map) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_map_update(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;
        let value = self.read_buffer(value_ptr)?;

        let mut map = match load_js_authored_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Owner-only: the collection returns `ActionNotAllowed` when the current
        // executor is not the stored owner; surface it verbatim.
        match map.update(&key, &value) {
            Ok(()) => match save_js_instance(&mut map) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(dest_register_id, message),
            },
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let mut map = match load_js_authored_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Owner-only: a non-owner remove yields `ActionNotAllowed`; an absent key
        // yields `Ok(None)`.
        match map.remove(&key) {
            Ok(Some(previous)) => {
                if let Err(message) = save_js_instance(&mut map) {
                    return self.write_error_message(dest_register_id, message);
                }
                self.write_register_bytes(dest_register_id, &previous)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_map_owner_of(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let map = match load_js_authored_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Present key → 32-byte owner public key in the register (status 1);
        // absent key → cleared register (status 0), matching `get`'s convention.
        match map.owner_of(&key) {
            Ok(Some(owner)) => {
                self.write_register_bytes(dest_register_id, &owner)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_map_owned_by_me(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let map = match load_js_authored_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(0, message),
        };

        match map.owned_by_me(&key) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_authored_map_len(
        &mut self,
        map_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let map = match load_js_authored_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match map.len() {
            Ok(len) => {
                let len_u64 = u64::try_from(len).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &len_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_vector_push(
        &mut self,
        vector_id_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut vector = match load_js_authored_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // `push` stamps the current executor as owner and returns the new index.
        match vector.push(&value) {
            Ok(index) => {
                if let Err(message) = save_js_instance(&mut vector) {
                    return self.write_error_message(dest_register_id, message);
                }
                let index_u64 = u64::try_from(index).map_err(|_| HostError::IntegerOverflow)?;
                self.write_register_bytes(dest_register_id, &index_u64.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_vector_update(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self.write_error_message(
                    dest_register_id,
                    format!("index {index} does not fit into usize"),
                )
            }
        };

        let value = self.read_buffer(value_ptr)?;

        let mut vector = match load_js_authored_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Owner-only: a non-owner update yields `ActionNotAllowed`.
        match vector.update(idx, &value) {
            Ok(()) => match save_js_instance(&mut vector) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(dest_register_id, message),
            },
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_vector_tombstone(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self.write_error_message(
                    dest_register_id,
                    format!("index {index} does not fit into usize"),
                )
            }
        };

        let mut vector = match load_js_authored_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Owner-only: a non-owner tombstone yields `ActionNotAllowed`.
        match vector.tombstone(idx) {
            Ok(()) => match save_js_instance(&mut vector) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(dest_register_id, message),
            },
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_vector_owner_of(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self.write_error_message(
                    dest_register_id,
                    format!("index {index} does not fit into usize"),
                )
            }
        };

        let vector = match load_js_authored_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Present slot → 32-byte owner public key (status 1); out-of-bounds slot
        // → cleared register (status 0), matching `get`'s convention.
        match vector.owner_of(idx) {
            Ok(Some(owner)) => {
                self.write_register_bytes(dest_register_id, &owner)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_authored_vector_owned_by_me(
        &mut self,
        vector_id_ptr: u64,
        index: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let idx = match usize::try_from(index) {
            Ok(value) => value,
            Err(_) => {
                return self
                    .write_error_message(0, format!("index {index} does not fit into usize"))
            }
        };

        let vector = match load_js_authored_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(0, message),
        };

        match vector.owned_by_me(idx) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_authored_vector_iter(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let vector = match load_js_authored_vector_instance(vector_id) {
            Ok(vector) => vector,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let values = match vector.iter() {
            Ok(values) => values,
            Err(err) => return self.write_error_message(dest_register_id, err),
        };

        let count = u32::try_from(values.len()).map_err(|_| HostError::IntegerOverflow)?;

        let mut total_len: usize = 4;
        for value in &values {
            let value_len = value.len();
            u32::try_from(value_len).map_err(|_| HostError::IntegerOverflow)?;
            total_len = total_len
                .checked_add(4)
                .and_then(|acc| acc.checked_add(value_len))
                .ok_or(HostError::IntegerOverflow)?;
        }

        let mut buffer = Vec::with_capacity(total_len);
        buffer.extend_from_slice(&count.to_le_bytes());
        for value in values {
            let value_len = u32::try_from(value.len()).map_err(|_| HostError::IntegerOverflow)?;
            buffer.extend_from_slice(&value_len.to_le_bytes());
            buffer.extend_from_slice(&value);
        }

        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_shared_new(
        &mut self,
        writers_ptr: u64,
        frozen: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let writers = match self.read_writer_set(writers_ptr)? {
            Ok(writers) => writers,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let frozen = frozen != 0;

        let outcome = panic::catch_unwind(AssertUnwindSafe(
            move || -> Result<JsSharedStorage, String> {
                let mut cell = JsSharedStorage::new(writers, frozen);
                save_js_instance(&mut cell)?;
                Ok(cell)
            },
        ));

        match outcome {
            Ok(Ok(cell)) => {
                self.write_register_bytes(dest_register_id, cell.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => self.write_error_message(
                dest_register_id,
                panic_payload_to_string(payload.as_ref(), "unknown panic"),
            ),
        }
    }

    fn crdt_shared_new_with_id(
        &mut self,
        id_ptr: u64,
        writers_ptr: u64,
        frozen: u32,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let id = match self.read_map_id(id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let writers = match self.read_writer_set(writers_ptr)? {
            Ok(writers) => writers,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };
        let frozen = frozen != 0;

        let outcome = panic::catch_unwind(AssertUnwindSafe(
            move || -> Result<JsSharedStorage, String> {
                let mut cell = JsSharedStorage::new_with_id(id, writers, frozen);
                save_js_instance(&mut cell)?;
                Ok(cell)
            },
        ));

        match outcome {
            Ok(Ok(cell)) => {
                self.write_register_bytes(dest_register_id, cell.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => self.write_error_message(
                dest_register_id,
                panic_payload_to_string(payload.as_ref(), "unknown panic"),
            ),
        }
    }

    fn crdt_shared_set(&mut self, cell_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
        let cell_id = match self.read_map_id(cell_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let value = self.read_buffer(value_ptr)?;

        let mut cell = match load_js_shared_instance(cell_id) {
            Ok(cell) => cell,
            Err(message) => return self.write_error_message(0, message),
        };

        // Writer-gated: `set` returns `ActionNotAllowed` when the current
        // executor is not in the writer set; surface it verbatim in register 0.
        match cell.set(&value) {
            Ok(()) => match save_js_instance(&mut cell) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_shared_get(&mut self, cell_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let cell_id = match self.read_map_id(cell_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let cell = match load_js_shared_instance(cell_id) {
            Ok(cell) => cell,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match cell.get() {
            Ok(Some(value)) => {
                self.write_register_bytes(dest_register_id, &value)?;
                Ok(1)
            }
            Ok(None) => {
                self.clear_register(dest_register_id)?;
                Ok(0)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    fn crdt_shared_writers(
        &mut self,
        cell_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let cell_id = match self.read_map_id(cell_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let cell = match load_js_shared_instance(cell_id) {
            Ok(cell) => cell,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        // Concatenated 32-byte keys; the JS side decodes `len / 32` entries.
        let writers = cell.writers();
        let mut buffer = Vec::with_capacity(writers.len().saturating_mul(PUBLIC_KEY_LEN));
        for writer in &writers {
            buffer.extend_from_slice(writer);
        }

        self.write_register_bytes(dest_register_id, &buffer)?;
        Ok(1)
    }

    fn crdt_shared_writable_by_me(&mut self, cell_id_ptr: u64) -> VMLogicResult<i32> {
        let cell_id = match self.read_map_id(cell_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let cell = match load_js_shared_instance(cell_id) {
            Ok(cell) => cell,
            Err(message) => return self.write_error_message(0, message),
        };

        Ok(i32::from(cell.writable_by_me()))
    }

    fn crdt_shared_is_frozen(&mut self, cell_id_ptr: u64) -> VMLogicResult<i32> {
        let cell_id = match self.read_map_id(cell_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let cell = match load_js_shared_instance(cell_id) {
            Ok(cell) => cell,
            Err(message) => return self.write_error_message(0, message),
        };

        Ok(i32::from(cell.is_frozen()))
    }

    fn crdt_shared_rotate_writers(
        &mut self,
        cell_id_ptr: u64,
        writers_ptr: u64,
    ) -> VMLogicResult<i32> {
        let cell_id = match self.read_map_id(cell_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let writers = match self.read_writer_set(writers_ptr)? {
            Ok(writers) => writers,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut cell = match load_js_shared_instance(cell_id) {
            Ok(cell) => cell,
            Err(message) => return self.write_error_message(0, message),
        };

        // Writer-gated: a non-writer rotation (or a frozen/empty target) yields
        // `ActionNotAllowed`; surface it verbatim in register 0.
        match cell.rotate_writers(writers) {
            Ok(()) => match save_js_instance(&mut cell) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    /// Delete a root-level collection entity by id and unlink it from the root.
    ///
    /// Used by the JS SDK's deterministic-id reassignment to reclaim the
    /// random-id collection that is orphaned when a top-level `@State` field is
    /// re-opened at its deterministic id. Every JS collection is created as a
    /// child of [`Id::root()`], so the orphan is always a root child.
    ///
    /// Delegates to [`Interface::remove_child_from`], which cascades the subtree,
    /// refuses to delete Frozen data (which peers would reject, causing a
    /// split-brain), and enforces writer authority for a `Shared` cell — so a
    /// caller can only delete a collection it legitimately created. The
    /// reassignment runs on fresh state, where the create and this delete land in
    /// the same genesis delta, so every replica converges with no orphan.
    ///
    /// Returns 1 if an entity was deleted, 0 if none existed at that id
    /// (idempotent), or -1 with an error message in `dest_register_id`.
    fn crdt_delete_collection(&mut self, id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let id = match self.read_map_id(id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        match Interface::<MainStorage>::remove_child_from(Id::root(), id) {
            Ok(true) => Ok(1),
            Ok(false) => Ok(0),
            Err(err) => self.write_error_message(dest_register_id, err.to_string()),
        }
    }

    /// Reads a writer set (a buffer of concatenated 32-byte public keys) from
    /// guest memory, returning a decoding error string if the length is not a
    /// multiple of 32.
    fn read_writer_set(&mut self, ptr: u64) -> VMLogicResult<Result<Vec<[u8; 32]>, String>> {
        let bytes = self.read_buffer(ptr)?;
        // Reject an empty writer set at the decode boundary. A cell created with
        // no writers could never be written to, and `rotate_writers` refuses an
        // empty set, so it could never be recovered — the cell would be bricked
        // for good. This mirrors the guard `WriterSetCell::rotate_writers` already
        // applies, closing the gap on the construction path (`shared_new` /
        // `shared_new_with_id`) that goes through this same decode helper.
        if bytes.is_empty() {
            return Ok(Err(
                "writer set must not be empty (a cell with no writers can never be \
                 written to or recovered)"
                    .to_string(),
            ));
        }
        if bytes.len() % PUBLIC_KEY_LEN != 0 {
            return Ok(Err(format!(
                "writer set must be a concatenation of {}-byte keys (received {} bytes)",
                PUBLIC_KEY_LEN,
                bytes.len()
            )));
        }

        let writers = bytes
            .chunks_exact(PUBLIC_KEY_LEN)
            .map(|chunk| {
                let mut key = [0u8; PUBLIC_KEY_LEN];
                key.copy_from_slice(chunk);
                key
            })
            .collect();
        Ok(Ok(writers))
    }

    fn read_map_id(&mut self, map_id_ptr: u64) -> VMLogicResult<Result<Id, String>> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(map_id_ptr)? };
        let data = self.read_guest_memory_slice(&buffer)?;

        if data.len() != COLLECTION_ID_LEN {
            return Ok(Err(format!(
                "mapId must be exactly {} bytes (received {})",
                COLLECTION_ID_LEN,
                data.len()
            )));
        }

        let mut bytes = [0u8; COLLECTION_ID_LEN];
        bytes.copy_from_slice(data);
        Ok(Ok(Id::new(bytes)))
    }

    fn read_buffer(&mut self, ptr: u64) -> VMLogicResult<Vec<u8>> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(ptr)? };
        Ok(self.read_guest_memory_slice(&buffer)?.to_vec())
    }

    fn write_register_bytes(&mut self, register_id: u64, bytes: &[u8]) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| logic.registers.set(logic.limits, register_id, bytes))
    }

    fn write_error_message(
        &mut self,
        register_id: u64,
        message: impl Display,
    ) -> VMLogicResult<i32> {
        let string = message.to_string();
        self.write_register_bytes(register_id, string.as_bytes())?;
        Ok(-1)
    }

    fn clear_register(&mut self, register_id: u64) -> VMLogicResult<()> {
        self.write_register_bytes(register_id, &[])
    }

    // ---- host-minted-id constructors, one per collection type ----
    js_new_op!(crdt_map_new, JsUnorderedMap, save: save_js_instance);
    js_new_op!(crdt_vector_new, JsVector, save: save_js_instance);
    js_new_op!(crdt_set_new, JsUnorderedSet, save: save_js_instance);
    js_new_op!(crdt_lww_new, JsLwwRegister, save: save_js_lww_instance);
    js_new_op!(crdt_counter_new, JsCounter, save: save_js_counter_instance);
    js_new_op!(user_storage_new, JsUserStorage, save: save_js_instance);
    js_new_op!(frozen_storage_new, JsFrozenStorage, save: save_js_instance);
    js_new_op!(crdt_pncounter_new, JsPnCounter, save: save_js_instance);
    js_new_op!(crdt_rga_new, JsRga, save: save_js_instance);
    js_new_op!(crdt_sortedmap_new, JsSortedMap, save: save_js_instance);
    js_new_op!(crdt_sortedset_new, JsSortedSet, save: save_js_instance);
    js_new_op!(crdt_authored_map_new, JsAuthoredMap, save: save_js_instance);
    js_new_op!(crdt_authored_vector_new, JsAuthoredVector, save: save_js_instance);

    // ---- deterministic-id constructors, one per collection type ----
    js_new_with_id_op!(crdt_map_new_with_id, JsUnorderedMap, save: save_js_instance);
    js_new_with_id_op!(crdt_vector_new_with_id, JsVector, save: save_js_instance);
    js_new_with_id_op!(crdt_set_new_with_id, JsUnorderedSet, save: save_js_instance);
    js_new_with_id_op!(crdt_lww_new_with_id, JsLwwRegister, save: save_js_lww_instance);
    js_new_with_id_op!(crdt_counter_new_with_id, JsCounter, save: save_js_counter_instance);
    js_new_with_id_op!(user_storage_new_with_id, JsUserStorage, save: save_js_instance);
    js_new_with_id_op!(frozen_storage_new_with_id, JsFrozenStorage, save: save_js_instance);
    js_new_with_id_op!(crdt_pncounter_new_with_id, JsPnCounter, save: save_js_instance);
    js_new_with_id_op!(crdt_rga_new_with_id, JsRga, save: save_js_instance);
    js_new_with_id_op!(crdt_sortedmap_new_with_id, JsSortedMap, save: save_js_instance);
    js_new_with_id_op!(crdt_sortedset_new_with_id, JsSortedSet, save: save_js_instance);
    js_new_with_id_op!(crdt_authored_map_new_with_id, JsAuthoredMap, save: save_js_instance);
    js_new_with_id_op!(crdt_authored_vector_new_with_id, JsAuthoredVector, save: save_js_instance);
}

fn load_js_lww_instance(id: Id) -> Result<JsLwwRegister, String> {
    load_js_lww_register_instance(id)
}

fn save_js_lww_instance(register: &mut JsLwwRegister) -> Result<(), String> {
    save_js_instance(register)
}

fn save_js_counter_instance(counter: &mut JsCounter) -> Result<(), String> {
    match counter.save() {
        Ok(_) => {
            let counter_id_str = counter.id().to_string();
            debug!(
                target: "runtime::counter",
                counter_id = %counter_id_str,
                "saved JsCounter to storage"
            );
            Ok(())
        }
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), counter) {
                Ok(_) => {
                    debug!(
                        target: "runtime::counter",
                        counter_id = %counter.id().to_string(),
                        "attached JsCounter to root index"
                    );
                    Ok(())
                }
                Err(StorageError::CannotCreateOrphan(_)) => Err("cannot create orphan".to_owned()),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Persist a JS-facing collection, attaching it to the root index if it has no
/// parent yet.
///
/// One function for every collection type. There were thirteen of these — one per
/// `Js*` type, byte-identical apart from the binding name — because each type's
/// `save()` is an inherent method, so nothing about them looked shared. It is:
/// `save()` is a one-line wrapper over `Interface::<MainStorage>::save`, which is
/// already generic over `Data`, and `add_child_to` takes the same bound. Thirteen
/// copies of an orphan-recovery path is thirteen places it can be fixed in twelve.
///
/// `CannotCreateOrphan` is not a failure here: it means the collection was minted
/// without a parent, which is exactly what a JS app does when it constructs one
/// standalone. Attaching it to the root and retrying is the recovery, and a second
/// `CannotCreateOrphan` after that is a genuine refusal.
fn save_js_instance<D: Data>(value: &mut D) -> Result<(), String> {
    match Interface::<MainStorage>::save(value) {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), value) {
                Ok(_) => Ok(()),
                Err(StorageError::CannotCreateOrphan(_)) => Err("cannot create orphan".to_owned()),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

/// Generate the load-or-recreate wrapper for one JS-facing collection type.
///
/// Thirteen of these were written out by hand, differing only in the type, the
/// tracing target, and the name of the id field in the log line — the three things
/// a macro can carry and a generic cannot, because a `target:` and a field name
/// must both be literals at the call site. What the copies could not keep straight
/// was the recovery path: seven of them logged the recreation and six silently did
/// it, which is the kind of divergence that makes a log trail unreadable exactly
/// when it is needed. Generated, they cannot disagree.
///
/// **The `Ok(None)` arm recreates rather than failing.** A JS app can serialise
/// just the collection id — via a state snapshot, say — while the underlying CRDT
/// was never persisted. Rebuilding the host object at the same id and attaching it
/// to the root makes the very next read or write behave as the app expects, instead
/// of failing on a collection the app believes it holds.
///
/// `JsSharedStorage` is deliberately **not** generated from this: a shared cell
/// cannot be recreated on the fly, because its writer set is only known at
/// construction and inventing an empty one would silently make the cell
/// unwritable. It stays hand-written below, with that reasoning beside it.
macro_rules! js_load_or_recreate {
    (
        $load:ident, $ty:ty,
        target: $target:literal,
        id_field: $id_field:ident,
        save: $save:ident $(,)?
    ) => {
        fn $load(id: Id) -> Result<$ty, String> {
            match <$ty>::load(id) {
                Ok(Some(value)) => {
                    debug!(
                        target: $target,
                        $id_field = %id.to_string(),
                        concat!("loaded ", stringify!($ty), " from storage")
                    );
                    Ok(value)
                }
                Ok(None) => {
                    let missing_id = id.to_string();
                    warn!(
                        target: $target,
                        $id_field = %missing_id,
                        concat!(stringify!($ty), " not found in storage")
                    );
                    let mut value = <$ty>::new_with_id(id);
                    match $save(&mut value) {
                        Ok(()) => {
                            debug!(
                                target: $target,
                                $id_field = %missing_id,
                                concat!("recreated missing ", stringify!($ty))
                            );
                            Ok(value)
                        }
                        Err(err) => Err(err),
                    }
                }
                Err(err) => Err(err.to_string()),
            }
        }
    };
}

js_load_or_recreate!(
    load_js_map_instance, JsUnorderedMap,
    target: "runtime::map",
    id_field: map_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_vector_instance, JsVector,
    target: "runtime::vector",
    id_field: vector_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_set_instance, JsUnorderedSet,
    target: "runtime::set",
    id_field: set_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_lww_register_instance, JsLwwRegister,
    target: "runtime::lww_register",
    id_field: register_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_counter_instance, JsCounter,
    target: "runtime::counter",
    id_field: counter_id,
    save: save_js_counter_instance,
);
js_load_or_recreate!(
    load_js_user_storage_instance, JsUserStorage,
    target: "runtime::user_storage",
    id_field: storage_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_frozen_storage_instance, JsFrozenStorage,
    target: "runtime::frozen_storage",
    id_field: storage_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_pncounter_instance, JsPnCounter,
    target: "runtime::pncounter",
    id_field: counter_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_rga_instance, JsRga,
    target: "runtime::rga",
    id_field: rga_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_sortedmap_instance, JsSortedMap,
    target: "runtime::sortedmap",
    id_field: map_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_sortedset_instance, JsSortedSet,
    target: "runtime::sortedset",
    id_field: set_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_authored_map_instance, JsAuthoredMap,
    target: "runtime::authored_map",
    id_field: map_id,
    save: save_js_instance,
);
js_load_or_recreate!(
    load_js_authored_vector_instance, JsAuthoredVector,
    target: "runtime::authored_vector",
    id_field: vector_id,
    save: save_js_instance,
);

fn ensure_root_index_internal() -> Result<(), StorageError> {
    match Index::<MainStorage>::get_hashes_for(Id::root()) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => {
            let timestamp = time_now();
            let metadata = Metadata::new(timestamp, timestamp);
            Index::<MainStorage>::add_root(ChildInfo::new(Id::root(), [0; 32], metadata))
        }
        Err(err) => Err(err),
    }
}

fn load_js_shared_instance(id: Id) -> Result<JsSharedStorage, String> {
    match JsSharedStorage::load(id) {
        Ok(Some(cell)) => {
            debug!(
                target: "runtime::shared_storage",
                cell_id = %id.to_string(),
                "loaded JsSharedStorage from storage"
            );
            Ok(cell)
        }
        Ok(None) => {
            let missing_id = id.to_string();
            warn!(
                target: "runtime::shared_storage",
                cell_id = %missing_id,
                "JsSharedStorage not found in storage"
            );
            // Unlike the other wrappers, a shared cell cannot be recreated on the
            // fly: its writer set is only known at construction, and inventing an
            // empty one would silently make the cell unwritable. A missing cell
            // means the id was referenced before `new`/`new_with_id` persisted it,
            // which is a caller error — surface it rather than paper over it.
            Err(format!("JsSharedStorage {missing_id} not found in storage"))
        }
        Err(err) => Err(err.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use crate::logic::{
        tests::{prepare_guest_buf_descriptor, setup_vm, SimpleMockStorage},
        Cow, VMContext, VMLimits, VMLogic, DIGEST_SIZE,
    };
    use wasmer::{AsStoreMut, Store};

    // Guest memory offsets used across the tests below. The `*_desc` offsets hold
    // 16-byte `{ ptr, len }` buffer descriptors; the `*_data` offsets hold payloads.
    const ID_DATA_PTR: u64 = 1000;
    const ID_DESC_PTR: u64 = 100;
    const KEY_DATA_PTR: u64 = 2000;
    const KEY_DESC_PTR: u64 = 200;
    const VALUE_DATA_PTR: u64 = 3000;
    const VALUE_DESC_PTR: u64 = 300;
    const WRITERS_DATA_PTR: u64 = 4000;
    const WRITERS_DESC_PTR: u64 = 400;

    /// Deleting a collection removes it from the root's children: the first
    /// delete reports it removed one entity, and a second delete of the same id
    /// is a no-op — proving the orphan is gone. Mirrors how the JS SDK reclaims
    /// the random-id collection orphaned by deterministic-id reassignment.
    #[test]
    fn test_js_crdt_delete_collection_removes_and_is_idempotent() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [9u8; 32];
        host.borrow_memory()
            .write(ID_DATA_PTR, &id)
            .expect("write id");
        prepare_guest_buf_descriptor(&host, ID_DESC_PTR, ID_DATA_PTR, id.len() as u64);

        // Create a collection at the id (a root child).
        assert_eq!(
            host.js_crdt_map_new_with_id(ID_DESC_PTR, 1).unwrap(),
            0,
            "constructor should succeed"
        );

        // First delete removes it.
        assert_eq!(
            host.js_crdt_delete_collection(ID_DESC_PTR, 2).unwrap(),
            1,
            "first delete removes the collection"
        );

        // Second delete finds nothing — idempotent, proving it was unlinked.
        assert_eq!(
            host.js_crdt_delete_collection(ID_DESC_PTR, 3).unwrap(),
            0,
            "second delete is a no-op"
        );
    }

    /// A `*_new_with_id` constructor must place the collection at exactly the
    /// caller-supplied id, and two handles built at the same id must address the
    /// same storage entity (insert via one, read via another).
    #[test]
    fn test_js_crdt_map_new_with_id_is_deterministic_and_shared() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // A known, non-random 32-byte id both nodes would derive independently.
        let id: [u8; 32] = [7u8; 32];
        host.borrow_memory()
            .write(ID_DATA_PTR, &id)
            .expect("write id bytes");
        prepare_guest_buf_descriptor(&host, ID_DESC_PTR, ID_DATA_PTR, id.len() as u64);

        // First handle at the deterministic id.
        let reg_a = 1u64;
        let res = host.js_crdt_map_new_with_id(ID_DESC_PTR, reg_a).unwrap();
        assert_eq!(res, 0, "constructor should succeed");
        assert_eq!(
            host.borrow_logic().registers.get(reg_a).unwrap(),
            &id,
            "returned id must equal the caller-supplied id"
        );

        // Second handle at the SAME id (as a different node/logical handle would).
        let reg_b = 2u64;
        let res = host.js_crdt_map_new_with_id(ID_DESC_PTR, reg_b).unwrap();
        assert_eq!(res, 0);
        assert_eq!(
            host.borrow_logic().registers.get(reg_b).unwrap(),
            &id,
            "second handle must resolve to the same id"
        );

        // Insert through the id, then read it back: both handles share one entity.
        let key = b"field";
        let value = b"payload";
        host.borrow_memory()
            .write(KEY_DATA_PTR, key)
            .expect("write key");
        prepare_guest_buf_descriptor(&host, KEY_DESC_PTR, KEY_DATA_PTR, key.len() as u64);
        host.borrow_memory()
            .write(VALUE_DATA_PTR, value)
            .expect("write value");
        prepare_guest_buf_descriptor(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, value.len() as u64);

        let reg_c = 3u64;
        let res = host
            .js_crdt_map_insert(ID_DESC_PTR, KEY_DESC_PTR, VALUE_DESC_PTR, reg_c)
            .unwrap();
        assert_eq!(res, 0, "inserting a new key returns 0 (no previous value)");

        let reg_d = 4u64;
        let res = host
            .js_crdt_map_get(ID_DESC_PTR, KEY_DESC_PTR, reg_d)
            .unwrap();
        assert_eq!(res, 1, "value must be found via the shared id");
        assert_eq!(
            host.borrow_logic().registers.get(reg_d).unwrap(),
            value,
            "round-tripped value must match what was inserted"
        );
    }

    /// Same guarantee for a second collection type (set), to cover a different
    /// save/load helper path.
    #[test]
    fn test_js_crdt_set_new_with_id_is_deterministic_and_shared() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [42u8; 32];
        host.borrow_memory()
            .write(ID_DATA_PTR, &id)
            .expect("write id bytes");
        prepare_guest_buf_descriptor(&host, ID_DESC_PTR, ID_DATA_PTR, id.len() as u64);

        let reg_a = 1u64;
        let res = host.js_crdt_set_new_with_id(ID_DESC_PTR, reg_a).unwrap();
        assert_eq!(res, 0);
        assert_eq!(host.borrow_logic().registers.get(reg_a).unwrap(), &id);

        // Second handle at the same id.
        let reg_b = 2u64;
        let res = host.js_crdt_set_new_with_id(ID_DESC_PTR, reg_b).unwrap();
        assert_eq!(res, 0);
        assert_eq!(host.borrow_logic().registers.get(reg_b).unwrap(), &id);

        let value = b"member";
        host.borrow_memory()
            .write(VALUE_DATA_PTR, value)
            .expect("write value");
        prepare_guest_buf_descriptor(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, value.len() as u64);

        let res = host
            .js_crdt_set_insert(ID_DESC_PTR, VALUE_DESC_PTR)
            .unwrap();
        assert_eq!(res, 1, "first insert of a value returns 1");

        // Reading membership via the shared id sees the insert.
        let res = host
            .js_crdt_set_contains(ID_DESC_PTR, VALUE_DESC_PTR)
            .unwrap();
        assert_eq!(res, 1, "value must be visible via the shared id");
    }

    /// Writes `bytes` at `data_ptr` and a `{ptr,len}` descriptor at `desc_ptr`.
    fn put_buffer(
        host: &crate::logic::VMHostFunctions<'_>,
        desc_ptr: u64,
        data_ptr: u64,
        bytes: &[u8],
    ) {
        host.borrow_memory()
            .write(data_ptr, bytes)
            .expect("write bytes");
        prepare_guest_buf_descriptor(host, desc_ptr, data_ptr, bytes.len() as u64);
    }

    /// Reads one `[len, bytes]` length-prefixed field starting at `*offset`,
    /// advancing `*offset` past it.
    fn read_field(buffer: &[u8], offset: &mut usize) -> Vec<u8> {
        let len = u32::from_le_bytes(buffer[*offset..*offset + 4].try_into().unwrap()) as usize;
        *offset += 4;
        let field = buffer[*offset..*offset + len].to_vec();
        *offset += len;
        field
    }

    /// Decodes the `[count][len,value]...` payload the set `*_iter` host fns emit.
    fn decode_set_items(buffer: &[u8]) -> Vec<Vec<u8>> {
        let count = u32::from_le_bytes(buffer[0..4].try_into().unwrap());
        let mut offset = 4usize;
        (0..count)
            .map(|_| read_field(buffer, &mut offset))
            .collect()
    }

    /// Decodes the `[count][klen,key,vlen,value]...` payload the map `*_iter` host
    /// fns emit (`count` is the number of entries, two fields each).
    fn decode_map_items(buffer: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let count = u32::from_le_bytes(buffer[0..4].try_into().unwrap());
        let mut offset = 4usize;
        (0..count)
            .map(|_| {
                let key = read_field(buffer, &mut offset);
                let value = read_field(buffer, &mut offset);
                (key, value)
            })
            .collect()
    }

    /// PN-counter must support increment AND decrement, and report a signed net.
    #[test]
    fn test_js_crdt_pncounter_increment_decrement_value() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [11u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);

        assert_eq!(
            host.js_crdt_pncounter_new_with_id(ID_DESC_PTR, 1).unwrap(),
            0
        );

        // +3, then -1 → net +2 for this (single) executor.
        for _ in 0..3 {
            assert_eq!(host.js_crdt_pncounter_increment(ID_DESC_PTR).unwrap(), 1);
        }
        assert_eq!(host.js_crdt_pncounter_decrement(ID_DESC_PTR).unwrap(), 1);

        let reg = 5u64;
        assert_eq!(host.js_crdt_pncounter_value(ID_DESC_PTR, reg).unwrap(), 1);
        let bytes = host.borrow_logic().registers.get(reg).unwrap();
        let value = i64::from_le_bytes(bytes.try_into().unwrap());
        assert_eq!(value, 2, "signed PN-counter value must be 3 - 1 = 2");

        // The per-executor net should match the total for a single writer.
        let reg2 = 6u64;
        assert_eq!(
            host.js_crdt_pncounter_get_executor_count(ID_DESC_PTR, 0, 0, reg2)
                .unwrap(),
            1
        );
        let bytes = host.borrow_logic().registers.get(reg2).unwrap();
        assert_eq!(i64::from_le_bytes(bytes.try_into().unwrap()), 2);
    }

    /// A second handle built at the same deterministic id must observe a mutation
    /// made through the first handle (the `*_new_with_id` determinism contract).
    #[test]
    fn test_js_crdt_pncounter_new_with_id_is_deterministic_and_shared() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [13u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);

        assert_eq!(
            host.js_crdt_pncounter_new_with_id(ID_DESC_PTR, 1).unwrap(),
            0
        );
        assert_eq!(host.borrow_logic().registers.get(1).unwrap(), &id);

        // Second handle at the SAME id.
        assert_eq!(
            host.js_crdt_pncounter_new_with_id(ID_DESC_PTR, 2).unwrap(),
            0
        );
        assert_eq!(host.borrow_logic().registers.get(2).unwrap(), &id);

        // Increment through the id, then read the value back via the shared id.
        assert_eq!(host.js_crdt_pncounter_increment(ID_DESC_PTR).unwrap(), 1);

        let reg = 5u64;
        assert_eq!(host.js_crdt_pncounter_value(ID_DESC_PTR, reg).unwrap(), 1);
        let bytes = host.borrow_logic().registers.get(reg).unwrap();
        assert_eq!(
            i64::from_le_bytes(bytes.try_into().unwrap()),
            1,
            "value must be visible via the shared id"
        );
    }

    /// RGA insert then read-back the whole text as UTF-8 bytes; length reflects
    /// the visible character count.
    #[test]
    fn test_js_crdt_rga_insert_get_text() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [21u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
        assert_eq!(host.js_crdt_rga_new_with_id(ID_DESC_PTR, 1).unwrap(), 0);

        // Insert "Hello" at position 0.
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"Hello");
        assert_eq!(
            host.js_crdt_rga_insert(ID_DESC_PTR, 0, VALUE_DESC_PTR)
                .unwrap(),
            1
        );

        // Append "!" at position 5 → "Hello!".
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"!");
        assert_eq!(
            host.js_crdt_rga_insert(ID_DESC_PTR, 5, VALUE_DESC_PTR)
                .unwrap(),
            1
        );

        let reg = 5u64;
        assert_eq!(host.js_crdt_rga_get_text(ID_DESC_PTR, reg).unwrap(), 1);
        assert_eq!(
            host.borrow_logic().registers.get(reg).unwrap(),
            b"Hello!",
            "RGA text round-trip must match inserts"
        );

        let reg2 = 6u64;
        assert_eq!(host.js_crdt_rga_len(ID_DESC_PTR, reg2).unwrap(), 1);
        let bytes = host.borrow_logic().registers.get(reg2).unwrap();
        assert_eq!(u64::from_le_bytes(bytes.try_into().unwrap()), 6);

        // Delete the first char → "ello!".
        assert_eq!(host.js_crdt_rga_delete(ID_DESC_PTR, 0).unwrap(), 1);
        let reg3 = 7u64;
        assert_eq!(host.js_crdt_rga_get_text(ID_DESC_PTR, reg3).unwrap(), 1);
        assert_eq!(host.borrow_logic().registers.get(reg3).unwrap(), b"ello!");
    }

    /// RGA `index`/`len` count Unicode scalar values (codepoints), NOT bytes.
    /// Exercises multi-byte characters (`é` = 2 bytes, `🎉` = 4 bytes) so a
    /// byte-indexed implementation would either land mid-codepoint (error) or
    /// miscount — this locks the documented codepoint semantics that the JS
    /// SDK wrapper depends on.
    #[test]
    fn test_js_crdt_rga_multibyte_codepoint_indexing() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [22u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
        assert_eq!(host.js_crdt_rga_new_with_id(ID_DESC_PTR, 1).unwrap(), 0);

        // "café" — 4 codepoints but 5 UTF-8 bytes (é is 2 bytes).
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, "café".as_bytes());
        assert_eq!(
            host.js_crdt_rga_insert(ID_DESC_PTR, 0, VALUE_DESC_PTR)
                .unwrap(),
            1
        );

        // len must be 4 (codepoints), not 5 (bytes).
        let reg = 5u64;
        assert_eq!(host.js_crdt_rga_len(ID_DESC_PTR, reg).unwrap(), 1);
        let bytes = host.borrow_logic().registers.get(reg).unwrap();
        assert_eq!(
            u64::from_le_bytes(bytes.try_into().unwrap()),
            4,
            "len counts codepoints, not bytes"
        );

        // Insert an astral-plane emoji (4 bytes, 1 codepoint) at codepoint
        // offset 4 (the end). Byte-index 4 would be mid-`é` and fail.
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, "🎉".as_bytes());
        assert_eq!(
            host.js_crdt_rga_insert(ID_DESC_PTR, 4, VALUE_DESC_PTR)
                .unwrap(),
            1
        );

        let reg2 = 6u64;
        assert_eq!(host.js_crdt_rga_get_text(ID_DESC_PTR, reg2).unwrap(), 1);
        assert_eq!(
            host.borrow_logic().registers.get(reg2).unwrap(),
            "café🎉".as_bytes(),
            "insert at codepoint offset 4 lands after é, before nothing"
        );

        // Delete codepoint 3 (`é`) → "caf🎉".
        assert_eq!(host.js_crdt_rga_delete(ID_DESC_PTR, 3).unwrap(), 1);
        let reg3 = 7u64;
        assert_eq!(host.js_crdt_rga_get_text(ID_DESC_PTR, reg3).unwrap(), 1);
        assert_eq!(
            host.borrow_logic().registers.get(reg3).unwrap(),
            "caf🎉".as_bytes(),
            "delete removes one codepoint by codepoint offset"
        );
    }

    /// SortedMap insert/get, and iteration must be in ascending key order
    /// regardless of insertion order.
    #[test]
    fn test_js_crdt_sortedmap_insert_get_and_ordered_iter() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [31u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
        assert_eq!(
            host.js_crdt_sortedmap_new_with_id(ID_DESC_PTR, 1).unwrap(),
            0
        );

        // Insert keys out of order: banana, apple, cherry.
        for (key, value) in [
            (b"banana".as_slice(), b"2".as_slice()),
            (b"apple".as_slice(), b"1".as_slice()),
            (b"cherry".as_slice(), b"3".as_slice()),
        ] {
            put_buffer(&host, KEY_DESC_PTR, KEY_DATA_PTR, key);
            put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, value);
            assert_eq!(
                host.js_crdt_sortedmap_insert(ID_DESC_PTR, KEY_DESC_PTR, VALUE_DESC_PTR, 9)
                    .unwrap(),
                0,
                "inserting a new key returns 0"
            );
        }

        // Point lookup.
        put_buffer(&host, KEY_DESC_PTR, KEY_DATA_PTR, b"apple");
        let reg = 5u64;
        assert_eq!(
            host.js_crdt_sortedmap_get(ID_DESC_PTR, KEY_DESC_PTR, reg)
                .unwrap(),
            1
        );
        assert_eq!(host.borrow_logic().registers.get(reg).unwrap(), b"1");

        // Ordered iteration: keys come back apple < banana < cherry.
        let reg2 = 6u64;
        assert_eq!(host.js_crdt_sortedmap_iter(ID_DESC_PTR, reg2).unwrap(), 1);
        let buffer = host.borrow_logic().registers.get(reg2).unwrap().to_vec();
        let entries = decode_map_items(&buffer);
        assert_eq!(
            entries,
            vec![
                (b"apple".to_vec(), b"1".to_vec()),
                (b"banana".to_vec(), b"2".to_vec()),
                (b"cherry".to_vec(), b"3".to_vec()),
            ],
            "SortedMap iteration must be in ascending key order"
        );
    }

    /// SortedSet insert then membership check via the shared id.
    #[test]
    fn test_js_crdt_sortedset_insert_contains() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [41u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
        assert_eq!(
            host.js_crdt_sortedset_new_with_id(ID_DESC_PTR, 1).unwrap(),
            0
        );

        // Insert out of order: gamma, alpha, beta.
        for value in [b"gamma".as_slice(), b"alpha".as_slice(), b"beta".as_slice()] {
            put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, value);
            assert_eq!(
                host.js_crdt_sortedset_insert(ID_DESC_PTR, VALUE_DESC_PTR)
                    .unwrap(),
                1,
                "first insert of a value returns 1"
            );
        }

        // Membership.
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"alpha");
        assert_eq!(
            host.js_crdt_sortedset_contains(ID_DESC_PTR, VALUE_DESC_PTR)
                .unwrap(),
            1
        );
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"missing");
        assert_eq!(
            host.js_crdt_sortedset_contains(ID_DESC_PTR, VALUE_DESC_PTR)
                .unwrap(),
            0
        );

        // Ordered iteration: alpha < beta < gamma.
        let reg = 6u64;
        assert_eq!(host.js_crdt_sortedset_iter(ID_DESC_PTR, reg).unwrap(), 1);
        let buffer = host.borrow_logic().registers.get(reg).unwrap().to_vec();
        assert_eq!(
            decode_set_items(&buffer),
            vec![b"alpha".to_vec(), b"beta".to_vec(), b"gamma".to_vec()],
            "SortedSet iteration must be in ascending order"
        );
    }

    /// AuthoredMap: an insert stamps the caller as owner (`owner_of` returns the
    /// executor identity, `owned_by_me` is true), and an `update` from a
    /// DIFFERENT executor is rejected with an ownership error. The non-owner
    /// path is exercised by building a second VM (with a different
    /// `executor_public_key`) over the SAME backing storage.
    #[test]
    fn test_js_crdt_authored_map_owner_stamp_and_non_owner_update_rejected() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let id: [u8; 32] = [61u8; 32];
        let alice: [u8; 32] = [0xA1; 32];
        let bob: [u8; 32] = [0xB0; 32];

        // --- Alice: create, insert, confirm ownership. ---
        {
            let context = VMContext::new(
                Cow::Owned(vec![]),
                [0u8; DIGEST_SIZE],
                alice,
                calimero_account::AccountId::from(alice),
            );
            let mut store = Store::default();
            let memory =
                wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
            let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
            let _ = logic.with_memory(memory);
            let mut host = logic.host_functions(store.as_store_mut());

            put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
            assert_eq!(
                host.js_crdt_authored_map_new_with_id(ID_DESC_PTR, 1)
                    .unwrap(),
                0
            );

            put_buffer(&host, KEY_DESC_PTR, KEY_DATA_PTR, b"apple");
            put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"1");
            assert_eq!(
                host.js_crdt_authored_map_insert(ID_DESC_PTR, KEY_DESC_PTR, VALUE_DESC_PTR, 2)
                    .unwrap(),
                0,
                "insert of a new key returns 0"
            );

            // Round-trip get.
            let reg = 3u64;
            assert_eq!(
                host.js_crdt_authored_map_get(ID_DESC_PTR, KEY_DESC_PTR, reg)
                    .unwrap(),
                1
            );
            assert_eq!(host.borrow_logic().registers.get(reg).unwrap(), b"1");

            // owner_of returns Alice's 32-byte identity.
            let reg2 = 4u64;
            assert_eq!(
                host.js_crdt_authored_map_owner_of(ID_DESC_PTR, KEY_DESC_PTR, reg2)
                    .unwrap(),
                1
            );
            assert_eq!(host.borrow_logic().registers.get(reg2).unwrap(), &alice);

            // owned_by_me is true for the inserter.
            assert_eq!(
                host.js_crdt_authored_map_owned_by_me(ID_DESC_PTR, KEY_DESC_PTR)
                    .unwrap(),
                1
            );
        }

        // --- Bob: a different executor cannot update Alice's entry. ---
        {
            let context = VMContext::new(
                Cow::Owned(vec![]),
                [0u8; DIGEST_SIZE],
                bob,
                calimero_account::AccountId::from(bob),
            );
            let mut store = Store::default();
            let memory =
                wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
            let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
            let _ = logic.with_memory(memory);
            let mut host = logic.host_functions(store.as_store_mut());

            put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
            put_buffer(&host, KEY_DESC_PTR, KEY_DATA_PTR, b"apple");
            put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"99");

            let reg = 1u64;
            let res = host
                .js_crdt_authored_map_update(ID_DESC_PTR, KEY_DESC_PTR, VALUE_DESC_PTR, reg)
                .unwrap();
            assert_eq!(res, -1, "non-owner update must be rejected");
            let msg = String::from_utf8(host.borrow_logic().registers.get(reg).unwrap().to_vec())
                .unwrap();
            assert!(
                msg.to_lowercase().contains("owner"),
                "error should mention ownership, got: {msg}"
            );

            // owned_by_me is false for Bob.
            assert_eq!(
                host.js_crdt_authored_map_owned_by_me(ID_DESC_PTR, KEY_DESC_PTR)
                    .unwrap(),
                0
            );

            // The original value is unchanged.
            let reg2 = 2u64;
            assert_eq!(
                host.js_crdt_authored_map_get(ID_DESC_PTR, KEY_DESC_PTR, reg2)
                    .unwrap(),
                1
            );
            assert_eq!(host.borrow_logic().registers.get(reg2).unwrap(), b"1");
        }
    }

    /// AuthoredVector: `push` returns the new index (u64 LE) and stamps the
    /// pusher as owner; `get`/`owner_of`/`owned_by_me` reflect that; a
    /// `tombstone` by the owner succeeds and preserves the slot's position.
    #[test]
    fn test_js_crdt_authored_vector_push_get_owner_and_tombstone() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [71u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
        assert_eq!(
            host.js_crdt_authored_vector_new_with_id(ID_DESC_PTR, 1)
                .unwrap(),
            0
        );

        // push returns the new index (u64 LE) in the register.
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"first");
        let reg = 2u64;
        assert_eq!(
            host.js_crdt_authored_vector_push(ID_DESC_PTR, VALUE_DESC_PTR, reg)
                .unwrap(),
            1
        );
        let bytes = host.borrow_logic().registers.get(reg).unwrap();
        assert_eq!(
            u64::from_le_bytes(bytes.try_into().unwrap()),
            0,
            "first push lands at index 0"
        );

        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"second");
        let reg_b = 3u64;
        assert_eq!(
            host.js_crdt_authored_vector_push(ID_DESC_PTR, VALUE_DESC_PTR, reg_b)
                .unwrap(),
            1
        );
        let bytes = host.borrow_logic().registers.get(reg_b).unwrap();
        assert_eq!(
            u64::from_le_bytes(bytes.try_into().unwrap()),
            1,
            "second push lands at index 1"
        );

        // get index 0.
        let reg_c = 4u64;
        assert_eq!(
            host.js_crdt_authored_vector_get(ID_DESC_PTR, 0, reg_c)
                .unwrap(),
            1
        );
        assert_eq!(host.borrow_logic().registers.get(reg_c).unwrap(), b"first");

        // owner_of(0) is the (default) executor identity.
        let reg_d = 5u64;
        assert_eq!(
            host.js_crdt_authored_vector_owner_of(ID_DESC_PTR, 0, reg_d)
                .unwrap(),
            1
        );
        assert_eq!(
            host.borrow_logic().registers.get(reg_d).unwrap(),
            &[0u8; 32]
        );

        // owned_by_me(0) is true for the pusher.
        assert_eq!(
            host.js_crdt_authored_vector_owned_by_me(ID_DESC_PTR, 0)
                .unwrap(),
            1
        );

        // tombstone by owner succeeds; the slot becomes empty but is retained.
        let reg_e = 6u64;
        assert_eq!(
            host.js_crdt_authored_vector_tombstone(ID_DESC_PTR, 0, reg_e)
                .unwrap(),
            1
        );
        let reg_f = 7u64;
        assert_eq!(
            host.js_crdt_authored_vector_get(ID_DESC_PTR, 0, reg_f)
                .unwrap(),
            1
        );
        assert_eq!(
            host.borrow_logic().registers.get(reg_f).unwrap(),
            b"",
            "tombstone overwrites the slot with an empty value"
        );

        // len is still 2 — tombstone preserves the slot's position.
        let reg_g = 8u64;
        assert_eq!(
            host.js_crdt_authored_vector_len(ID_DESC_PTR, reg_g)
                .unwrap(),
            1
        );
        let bytes = host.borrow_logic().registers.get(reg_g).unwrap();
        assert_eq!(u64::from_le_bytes(bytes.try_into().unwrap()), 2);
    }

    /// A `*_new_with_id` constructor must place the authored vector at exactly
    /// the caller-supplied id, and two handles at the same id must address the
    /// same storage entity (push via one, read via another).
    #[test]
    fn test_js_crdt_authored_vector_new_with_id_is_deterministic_and_shared() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [81u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);

        // First handle at the deterministic id.
        let reg_a = 1u64;
        assert_eq!(
            host.js_crdt_authored_vector_new_with_id(ID_DESC_PTR, reg_a)
                .unwrap(),
            0
        );
        assert_eq!(host.borrow_logic().registers.get(reg_a).unwrap(), &id);

        // Second handle at the SAME id.
        let reg_b = 2u64;
        assert_eq!(
            host.js_crdt_authored_vector_new_with_id(ID_DESC_PTR, reg_b)
                .unwrap(),
            0
        );
        assert_eq!(host.borrow_logic().registers.get(reg_b).unwrap(), &id);

        // Push through the id, then read back via the shared id.
        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"payload");
        let reg_c = 3u64;
        assert_eq!(
            host.js_crdt_authored_vector_push(ID_DESC_PTR, VALUE_DESC_PTR, reg_c)
                .unwrap(),
            1
        );

        let reg_d = 4u64;
        assert_eq!(
            host.js_crdt_authored_vector_get(ID_DESC_PTR, 0, reg_d)
                .unwrap(),
            1,
            "value must be found via the shared id"
        );
        assert_eq!(
            host.borrow_logic().registers.get(reg_d).unwrap(),
            b"payload"
        );
    }

    /// Regression for calimero-sdk-js#88 (`allPosts failed: BorshReader:
    /// unexpected end of input`): a value pushed to an `AuthoredVector` must
    /// come back byte-identical through `iter`, with the `[count][len,value]`
    /// framing wrapping exactly the pushed bytes (no owner/metadata bleed).
    #[test]
    fn test_js_crdt_authored_vector_iter_roundtrips_value_bytes() {
        // Exact bytes `serialize("hello")` produces on the JS side (borsh
        // string: len:u32 LE + utf8 bytes).
        const HELLO_BORSH: &[u8] = &[0x05, 0x00, 0x00, 0x00, b'h', b'e', b'l', b'l', b'o'];

        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let id: [u8; 32] = [91u8; 32];
        put_buffer(&host, ID_DESC_PTR, ID_DATA_PTR, &id);
        assert_eq!(
            host.js_crdt_authored_vector_new_with_id(ID_DESC_PTR, 1)
                .unwrap(),
            0
        );

        put_buffer(&host, VALUE_DESC_PTR, VALUE_DATA_PTR, HELLO_BORSH);
        assert_eq!(
            host.js_crdt_authored_vector_push(ID_DESC_PTR, VALUE_DESC_PTR, 2)
                .unwrap(),
            1
        );

        let reg = 3u64;
        assert_eq!(
            host.js_crdt_authored_vector_iter(ID_DESC_PTR, reg).unwrap(),
            1
        );
        let buffer = host.borrow_logic().registers.get(reg).unwrap().to_vec();
        let items = decode_set_items(&buffer);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].as_slice(),
            HELLO_BORSH,
            "AuthoredVector iter must return exactly the pushed value bytes"
        );

        // --- Root cause of calimero-sdk-js#88 -------------------------------
        // `deletePost` -> `tombstone`, which core implements as
        // `update(index, V::default())`. For the byte-oriented wrapper
        // (`AuthoredVector<Vec<u8>>`) `Vec::<u8>::default()` is the EMPTY byte
        // string, so the slot is overwritten with zero bytes. `iter` faithfully
        // returns that empty slot — a byte-identical round-trip — but the JS
        // `deserialize` then reads a self-describing value out of ZERO bytes and
        // throws `BorshReader: unexpected end of input`. This asserts the exact
        // empty-slot bytes that the JS decoder chokes on; the corruption is NOT
        // in the value round-trip, it is the empty tombstone representation.
        assert_eq!(
            host.js_crdt_authored_vector_tombstone(ID_DESC_PTR, 0, 4)
                .unwrap(),
            1,
            "owner tombstone must succeed"
        );

        let reg2 = 5u64;
        assert_eq!(
            host.js_crdt_authored_vector_iter(ID_DESC_PTR, reg2)
                .unwrap(),
            1
        );
        let buffer = host.borrow_logic().registers.get(reg2).unwrap().to_vec();
        let items = decode_set_items(&buffer);
        assert_eq!(items.len(), 1, "tombstoned slot is retained by iter");
        assert!(
            items[0].is_empty(),
            "tombstoned slot is returned as ZERO value bytes — JS deserialize \
             then hits end-of-input trying to decode a self-describing value \
             out of nothing (calimero-sdk-js#88)"
        );
    }

    /// Concatenates 32-byte public keys into the writer-set ABI buffer.
    fn writers_buf(keys: &[[u8; 32]]) -> Vec<u8> {
        keys.iter().flat_map(|k| k.iter().copied()).collect()
    }

    /// Builds a fresh `VMLogic`/host over `storage` with `executor` as the
    /// current identity. The returned `Store` must be kept alive alongside the
    /// closure's use of the host (mirrors the AuthoredMap non-owner test).
    macro_rules! shared_host {
        ($storage:expr, $limits:expr, $executor:expr, $body:expr) => {{
            // The account is derived from the executor so distinct test writers get
            // distinct accounts, matching today's one-account-per-writer reality.
            let context = VMContext::new(
                Cow::Owned(vec![]),
                [0u8; DIGEST_SIZE],
                $executor,
                calimero_account::AccountId::from($executor),
            );
            let mut store = Store::default();
            let memory =
                wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
            let mut logic = VMLogic::new($storage, None, context, $limits, None);
            let _ = logic.with_memory(memory);
            let mut host = logic.host_functions(store.as_store_mut());
            #[allow(clippy::redundant_closure_call)]
            $body(&mut host)
        }};
    }

    /// SharedStorage: a writer can set/get, `writers()` reflects the initial set,
    /// `writable_by_me` is true for the writer, and `is_frozen` reflects the
    /// constructor flag.
    #[test]
    fn test_js_crdt_shared_set_get_writers_writable_and_frozen() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let alice: [u8; 32] = [0xA1; 32];
        let id: [u8; 32] = [0x51; 32];

        shared_host!(
            &mut storage,
            &limits,
            alice,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                put_buffer(
                    host,
                    WRITERS_DESC_PTR,
                    WRITERS_DATA_PTR,
                    &writers_buf(&[alice]),
                );

                // Construct an unfrozen cell writable by alice.
                assert_eq!(
                    host.js_crdt_shared_new_with_id(ID_DESC_PTR, WRITERS_DESC_PTR, 0, 1)
                        .unwrap(),
                    0,
                    "constructor should succeed"
                );
                assert_eq!(
                    host.borrow_logic().registers.get(1).unwrap(),
                    &id,
                    "returned id must equal the caller-supplied id"
                );

                // alice is a writer.
                assert_eq!(host.js_crdt_shared_writable_by_me(ID_DESC_PTR).unwrap(), 1);
                // not frozen.
                assert_eq!(host.js_crdt_shared_is_frozen(ID_DESC_PTR).unwrap(), 0);

                // set → get round-trip.
                put_buffer(host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"hello");
                assert_eq!(
                    host.js_crdt_shared_set(ID_DESC_PTR, VALUE_DESC_PTR)
                        .unwrap(),
                    1,
                    "writer set must succeed"
                );
                let reg = 2u64;
                assert_eq!(host.js_crdt_shared_get(ID_DESC_PTR, reg).unwrap(), 1);
                assert_eq!(host.borrow_logic().registers.get(reg).unwrap(), b"hello");

                // writers() returns the initial set (one 32-byte key = alice).
                let reg2 = 3u64;
                assert_eq!(host.js_crdt_shared_writers(ID_DESC_PTR, reg2).unwrap(), 1);
                assert_eq!(host.borrow_logic().registers.get(reg2).unwrap(), &alice);
            }
        );
    }

    /// A cell must not be constructible with an empty writer set: with no
    /// writers, `set`/`rotate_writers` could never succeed (rotate refuses an
    /// empty set), so the cell would be permanently bricked. Both constructors
    /// decode the writer buffer via `read_writer_set`, which rejects it.
    #[test]
    fn test_js_crdt_shared_empty_writer_set_rejected() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let alice: [u8; 32] = [0xA1; 32];
        let id: [u8; 32] = [0x52; 32];

        shared_host!(
            &mut storage,
            &limits,
            alice,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                // Empty writer buffer (zero keys).
                put_buffer(host, WRITERS_DESC_PTR, WRITERS_DATA_PTR, &writers_buf(&[]));

                // new_with_id must refuse and surface an error (-1), not brick a cell.
                let reg = 1u64;
                assert_eq!(
                    host.js_crdt_shared_new_with_id(ID_DESC_PTR, WRITERS_DESC_PTR, 0, reg)
                        .unwrap(),
                    -1,
                    "empty writer set must be rejected"
                );
                let message =
                    String::from_utf8(host.borrow_logic().registers.get(reg).unwrap().to_vec())
                        .unwrap();
                assert!(
                    message.to_lowercase().contains("empty"),
                    "error should explain the empty writer set, got: {message}"
                );

                // The random-id constructor rejects it too.
                assert_eq!(
                    host.js_crdt_shared_new(WRITERS_DESC_PTR, 0, 2u64).unwrap(),
                    -1,
                    "empty writer set must be rejected by shared_new as well"
                );
            }
        );
    }

    /// SharedStorage writer-gating: a non-writer's `set` is rejected with an
    /// ownership/writer error and `writable_by_me` is false; after the writer
    /// rotates them in, the new writer can set. The non-writer path is exercised
    /// by a second VM (different `executor_public_key`) over the SAME storage.
    #[test]
    fn test_js_crdt_shared_non_writer_rejected_then_rotation_grants() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let alice: [u8; 32] = [0xA1; 32];
        let bob: [u8; 32] = [0xB0; 32];
        let id: [u8; 32] = [0x52; 32];

        // --- Alice: create with herself as sole writer, set a value. ---
        shared_host!(
            &mut storage,
            &limits,
            alice,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                put_buffer(
                    host,
                    WRITERS_DESC_PTR,
                    WRITERS_DATA_PTR,
                    &writers_buf(&[alice]),
                );
                assert_eq!(
                    host.js_crdt_shared_new_with_id(ID_DESC_PTR, WRITERS_DESC_PTR, 0, 1)
                        .unwrap(),
                    0
                );
                put_buffer(host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"v1");
                assert_eq!(
                    host.js_crdt_shared_set(ID_DESC_PTR, VALUE_DESC_PTR)
                        .unwrap(),
                    1
                );
            }
        );

        // --- Bob: not a writer — cannot set, and writable_by_me is false. ---
        shared_host!(
            &mut storage,
            &limits,
            bob,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                assert_eq!(host.js_crdt_shared_writable_by_me(ID_DESC_PTR).unwrap(), 0);

                put_buffer(host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"v2");
                let res = host
                    .js_crdt_shared_set(ID_DESC_PTR, VALUE_DESC_PTR)
                    .unwrap();
                assert_eq!(res, -1, "non-writer set must be rejected");
                let msg = String::from_utf8(host.borrow_logic().registers.get(0).unwrap().to_vec())
                    .unwrap();
                // The `PermissionedStorage` API gate rejects first ("Action not
                // allowed: Executor is not authorised …"), before the inner
                // `WriterSetCell`'s writer-specific message.
                assert!(
                    msg.to_lowercase().contains("not allowed")
                        || msg.to_lowercase().contains("authoris"),
                    "error should signal an authorization failure, got: {msg}"
                );

                // The value is unchanged.
                let reg = 1u64;
                assert_eq!(host.js_crdt_shared_get(ID_DESC_PTR, reg).unwrap(), 1);
                assert_eq!(host.borrow_logic().registers.get(reg).unwrap(), b"v1");
            }
        );

        // --- Alice: rotate the writer set to add Bob. ---
        shared_host!(
            &mut storage,
            &limits,
            alice,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                put_buffer(
                    host,
                    WRITERS_DESC_PTR,
                    WRITERS_DATA_PTR,
                    &writers_buf(&[alice, bob]),
                );
                assert_eq!(
                    host.js_crdt_shared_rotate_writers(ID_DESC_PTR, WRITERS_DESC_PTR)
                        .unwrap(),
                    1,
                    "current writer may rotate"
                );
            }
        );

        // --- Bob: now a writer — can set. ---
        shared_host!(
            &mut storage,
            &limits,
            bob,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                assert_eq!(
                    host.js_crdt_shared_writable_by_me(ID_DESC_PTR).unwrap(),
                    1,
                    "bob is a writer after rotation"
                );
                put_buffer(host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"v2");
                assert_eq!(
                    host.js_crdt_shared_set(ID_DESC_PTR, VALUE_DESC_PTR)
                        .unwrap(),
                    1,
                    "new writer may set"
                );
                let reg = 1u64;
                assert_eq!(host.js_crdt_shared_get(ID_DESC_PTR, reg).unwrap(), 1);
                assert_eq!(host.borrow_logic().registers.get(reg).unwrap(), b"v2");
            }
        );
    }

    /// SharedStorage: a cell constructed `frozen` reports `is_frozen` and rejects
    /// any writer-set rotation.
    #[test]
    fn test_js_crdt_shared_frozen_blocks_rotation() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let alice: [u8; 32] = [0xA1; 32];
        let bob: [u8; 32] = [0xB0; 32];
        let id: [u8; 32] = [0x53; 32];

        shared_host!(
            &mut storage,
            &limits,
            alice,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                put_buffer(
                    host,
                    WRITERS_DESC_PTR,
                    WRITERS_DATA_PTR,
                    &writers_buf(&[alice]),
                );
                // frozen = 1.
                assert_eq!(
                    host.js_crdt_shared_new_with_id(ID_DESC_PTR, WRITERS_DESC_PTR, 1, 1)
                        .unwrap(),
                    0
                );
                assert_eq!(host.js_crdt_shared_is_frozen(ID_DESC_PTR).unwrap(), 1);

                put_buffer(
                    host,
                    WRITERS_DESC_PTR,
                    WRITERS_DATA_PTR,
                    &writers_buf(&[alice, bob]),
                );
                let res = host
                    .js_crdt_shared_rotate_writers(ID_DESC_PTR, WRITERS_DESC_PTR)
                    .unwrap();
                assert_eq!(res, -1, "rotation on a frozen cell must fail");
                let msg = String::from_utf8(host.borrow_logic().registers.get(0).unwrap().to_vec())
                    .unwrap();
                assert!(
                    msg.to_lowercase().contains("frozen"),
                    "error should mention frozen, got: {msg}"
                );
            }
        );
    }

    /// A `*_new_with_id` constructor must place the shared cell at exactly the
    /// caller-supplied id, and two handles built at the same id must address the
    /// same storage entity (set via one, read via another).
    #[test]
    fn test_js_crdt_shared_new_with_id_is_deterministic_and_shared() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let alice: [u8; 32] = [0xA1; 32];
        let id: [u8; 32] = [0x54; 32];

        shared_host!(
            &mut storage,
            &limits,
            alice,
            |host: &mut crate::logic::VMHostFunctions<'_>| {
                put_buffer(host, ID_DESC_PTR, ID_DATA_PTR, &id);
                put_buffer(
                    host,
                    WRITERS_DESC_PTR,
                    WRITERS_DATA_PTR,
                    &writers_buf(&[alice]),
                );

                // First handle at the deterministic id.
                let reg_a = 1u64;
                assert_eq!(
                    host.js_crdt_shared_new_with_id(ID_DESC_PTR, WRITERS_DESC_PTR, 0, reg_a)
                        .unwrap(),
                    0
                );
                assert_eq!(host.borrow_logic().registers.get(reg_a).unwrap(), &id);

                // Second handle at the SAME id.
                let reg_b = 2u64;
                assert_eq!(
                    host.js_crdt_shared_new_with_id(ID_DESC_PTR, WRITERS_DESC_PTR, 0, reg_b)
                        .unwrap(),
                    0
                );
                assert_eq!(host.borrow_logic().registers.get(reg_b).unwrap(), &id);

                // Set through the id, then read back via the shared id.
                put_buffer(host, VALUE_DESC_PTR, VALUE_DATA_PTR, b"payload");
                assert_eq!(
                    host.js_crdt_shared_set(ID_DESC_PTR, VALUE_DESC_PTR)
                        .unwrap(),
                    1
                );
                let reg_c = 3u64;
                assert_eq!(
                    host.js_crdt_shared_get(ID_DESC_PTR, reg_c).unwrap(),
                    1,
                    "value must be found via the shared id"
                );
                assert_eq!(
                    host.borrow_logic().registers.get(reg_c).unwrap(),
                    b"payload"
                );
            }
        );
    }
}
