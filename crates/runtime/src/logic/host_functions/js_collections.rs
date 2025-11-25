use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicResult},
};
use calimero_storage::{
    address::Id,
    entities::{ChildInfo, Metadata},
    env::{time_now, with_runtime_env, RuntimeEnv},
    index::Index,
    interface::{Interface, StorageError},
    js::{JsCounter, JsLwwRegister, JsUnorderedMap, JsUnorderedSet, JsVector},
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

impl VMHostFunctions<'_> {
    fn make_runtime_env(&mut self) -> VMLogicResult<RuntimeEnv> {
        self.with_logic_mut(|logic| {
            Ok(build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
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

    fn crdt_map_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        let outcome =
            panic::catch_unwind(AssertUnwindSafe(|| -> Result<JsUnorderedMap, String> {
                let mut map = JsUnorderedMap::new();
                save_js_map_instance(&mut map)?;
                Ok(map)
            }));

        match outcome {
            Ok(Ok(map)) => {
                self.write_register_bytes(dest_register_id, map.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => {
                self.write_error_message(dest_register_id, panic_payload_to_string(payload))
            }
        }
    }

    fn crdt_map_get(
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

        let map = match load_js_map_instance(map_id) {
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
                if let Err(message) = save_js_map_instance(&mut map) {
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
                if let Err(message) = save_js_map_instance(&mut map) {
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

    fn crdt_map_contains(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let key = self.read_buffer(key_ptr)?;

        let map = match load_js_map_instance(map_id) {
            Ok(map) => map,
            Err(message) => return self.write_error_message(0, message),
        };

        match map.contains(&key) {
            Ok(result) => Ok(i32::from(result)),
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_map_iter(&mut self, map_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let map_id = match self.read_map_id(map_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let map = match load_js_map_instance(map_id) {
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

    fn crdt_vector_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<JsVector, String> {
            let mut vector = JsVector::new();
            save_js_vector_instance(&mut vector)?;
            Ok(vector)
        }));

        match outcome {
            Ok(Ok(vector)) => {
                self.write_register_bytes(dest_register_id, vector.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => {
                self.write_error_message(dest_register_id, panic_payload_to_string(payload))
            }
        }
    }

    fn crdt_vector_len(&mut self, vector_id_ptr: u64, dest_register_id: u64) -> VMLogicResult<i32> {
        let vector_id = match self.read_map_id(vector_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let vector = match load_js_vector_instance(vector_id) {
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
            Ok(()) => match save_js_vector_instance(&mut vector) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_vector_get(
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

        let vector = match load_js_vector_instance(vector_id) {
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
                if let Err(message) = save_js_vector_instance(&mut vector) {
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

    fn crdt_set_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        let outcome =
            panic::catch_unwind(AssertUnwindSafe(|| -> Result<JsUnorderedSet, String> {
                let mut set = JsUnorderedSet::new();
                save_js_set_instance(&mut set)?;
                Ok(set)
            }));

        match outcome {
            Ok(Ok(set)) => {
                self.write_register_bytes(dest_register_id, set.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => {
                self.write_error_message(dest_register_id, panic_payload_to_string(payload))
            }
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
                if let Err(message) = save_js_set_instance(&mut set) {
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
                if let Err(message) = save_js_set_instance(&mut set) {
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
                if let Err(message) = save_js_set_instance(&mut set) {
                    return self.write_error_message(0, message);
                }
                Ok(1)
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    fn crdt_lww_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<JsLwwRegister, String> {
            let mut register = JsLwwRegister::new();
            save_js_lww_instance(&mut register)?;
            Ok(register)
        }));

        match outcome {
            Ok(Ok(mut register)) => {
                self.write_register_bytes(dest_register_id, register.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => {
                self.write_error_message(dest_register_id, panic_payload_to_string(payload))
            }
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

    fn crdt_counter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<JsCounter, String> {
            let mut counter = JsCounter::new();
            save_js_counter_instance(&mut counter)?;
            Ok(counter)
        }));

        match outcome {
            Ok(Ok(counter)) => {
                self.write_register_bytes(dest_register_id, counter.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => {
                self.write_error_message(dest_register_id, panic_payload_to_string(payload))
            }
        }
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

    fn read_map_id(&mut self, map_id_ptr: u64) -> VMLogicResult<Result<Id, String>> {
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(map_id_ptr)? };
        let data = self.read_guest_memory_slice(&buffer);

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
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(ptr)? };
        Ok(self.read_guest_memory_slice(&buffer).to_vec())
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
}

fn load_js_map_instance(id: Id) -> Result<JsUnorderedMap, String> {
    match JsUnorderedMap::load(id) {
        Ok(Some(map)) => {
            debug!(
                target: "runtime::map",
                map_id = %id.to_string(),
                "loaded JsUnorderedMap from storage"
            );
            Ok(map)
        }
        Ok(None) => {
            let missing_id = id.to_string();
            warn!(
                target: "runtime::map",
                map_id = %missing_id,
                "JsUnorderedMap not found in storage"
            );
            // This can happen if the contract serialised only the collection id
            // (e.g. via state snapshot) but the underlying CRDT was never
            // persisted.  Recreate the host object with the same id and attach
            // it to the root so the very next read/write works as expected.
            let mut map = JsUnorderedMap::new_with_id(id);
            match save_js_map_instance(&mut map) {
                Ok(()) => {
                    debug!(
                        target: "runtime::map",
                        map_id = %missing_id,
                        "recreated missing JsUnorderedMap"
                    );
                    Ok(map)
                }
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn save_js_map_instance(map: &mut JsUnorderedMap) -> Result<(), String> {
    match map.save() {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), map) {
                Ok(_) => Ok(()),
                Err(StorageError::CannotCreateOrphan(_)) => Err("cannot create orphan".to_owned()),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn load_js_vector_instance(id: Id) -> Result<JsVector, String> {
    match JsVector::load(id) {
        Ok(Some(vector)) => {
            debug!(
                target: "runtime::vector",
                vector_id = %id.to_string(),
                "loaded JsVector from storage"
            );
            Ok(vector)
        }
        Ok(None) => {
            let missing_id = id.to_string();
            warn!(
                target: "runtime::vector",
                vector_id = %missing_id,
                "JsVector not found in storage"
            );
            // The vector was referenced by id but not stored. Recreate and
            // persist it so subsequent operations proceed without errors.
            let mut vector = JsVector::new_with_id(id);
            match save_js_vector_instance(&mut vector) {
                Ok(()) => {
                    debug!(
                        target: "runtime::vector",
                        vector_id = %missing_id,
                        "recreated missing JsVector"
                    );
                    Ok(vector)
                }
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn save_js_vector_instance(vector: &mut JsVector) -> Result<(), String> {
    match vector.save() {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), vector) {
                Ok(_) => Ok(()),
                Err(StorageError::CannotCreateOrphan(_)) => Err("cannot create orphan".to_owned()),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn load_js_set_instance(id: Id) -> Result<JsUnorderedSet, String> {
    match JsUnorderedSet::load(id) {
        Ok(Some(set)) => {
            debug!(
                target: "runtime::set",
                set_id = %id.to_string(),
                "loaded JsUnorderedSet from storage"
            );
            Ok(set)
        }
        Ok(None) => {
            let missing_id = id.to_string();
            warn!(
                target: "runtime::set",
                set_id = %missing_id,
                "JsUnorderedSet not found in storage"
            );
            // See comment above: recreate the CRDT so the deserialised state
            // has a concrete backing object before we try to mutate it.
            let mut set = JsUnorderedSet::new_with_id(id);
            match save_js_set_instance(&mut set) {
                Ok(()) => {
                    debug!(
                        target: "runtime::set",
                        set_id = %missing_id,
                        "recreated missing JsUnorderedSet"
                    );
                    Ok(set)
                }
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn save_js_set_instance(set: &mut JsUnorderedSet) -> Result<(), String> {
    match set.save() {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), set) {
                Ok(_) => Ok(()),
                Err(StorageError::CannotCreateOrphan(_)) => Err("cannot create orphan".to_owned()),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn load_js_lww_register_instance(id: Id) -> Result<JsLwwRegister, String> {
    match JsLwwRegister::load(id) {
        Ok(Some(register)) => {
            debug!(
                target: "runtime::lww_register",
                register_id = %id.to_string(),
                "loaded JsLwwRegister from storage"
            );
            Ok(register)
        }
        Ok(None) => {
            let missing_id = id.to_string();
            warn!(
                target: "runtime::lww_register",
                register_id = %missing_id,
                "JsLwwRegister not found in storage"
            );
            let mut register = JsLwwRegister::new_with_id(id);
            match save_js_lww_register_instance(&mut register) {
                Ok(()) => {
                    debug!(
                        target: "runtime::lww_register",
                        register_id = %missing_id,
                        "recreated missing JsLwwRegister"
                    );
                    Ok(register)
                }
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn save_js_lww_register_instance(register: &mut JsLwwRegister) -> Result<(), String> {
    match register.save() {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), register) {
                Ok(_) => Ok(()),
                Err(StorageError::CannotCreateOrphan(_)) => Err("cannot create orphan".to_owned()),
                Err(err) => Err(err.to_string()),
            }
        }
        Err(err) => Err(err.to_string()),
    }
}

fn load_js_lww_instance(id: Id) -> Result<JsLwwRegister, String> {
    load_js_lww_register_instance(id)
}

fn save_js_lww_instance(register: &mut JsLwwRegister) -> Result<(), String> {
    save_js_lww_register_instance(register)
}

fn load_js_counter_instance(id: Id) -> Result<JsCounter, String> {
    match JsCounter::load(id) {
        Ok(Some(counter)) => {
            let counter_id_str = counter.id().to_string();
            debug!(
                target: "runtime::counter",
                counter_id = %counter_id_str,
                "loaded JsCounter from storage"
            );
            Ok(counter)
        }
        Ok(None) => {
            let missing_id = id.to_string();
            warn!(
                target: "runtime::counter",
                counter_id = %missing_id,
                "JsCounter not found in storage"
            );
            let mut counter = JsCounter::new_with_id(id);
            match save_js_counter_instance(&mut counter) {
                Ok(()) => {
                    debug!(
                        target: "runtime::counter",
                        counter_id = %missing_id,
                        "recreated missing JsCounter"
                    );
                    Ok(counter)
                }
                Err(err) => Err(err),
            }
        }
        Err(err) => Err(err.to_string()),
    }
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

fn panic_payload_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_owned()
    }
}
