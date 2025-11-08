use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicResult},
};
use calimero_storage::{
    address::Id,
    entities::{ChildInfo, Metadata},
    env::time_now,
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
use tracing::{debug, trace};

const COLLECTION_ID_LEN: usize = 32;

impl VMHostFunctions<'_> {
    /// Reads a value from the persistent storage.
    ///
    /// If the key exists, the corresponding value is copied into the specified register.
    ///
    /// # Arguments
    ///
    /// * `src_key_ptr` - A pointer to the key source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory where
    /// to place the value (if found).
    ///
    /// # Returns
    ///
    /// * Returns `1` if the key was found and the value was recorded into the register.
    /// * Returns `0` if the key was not found.
    ///
    /// # Errors
    ///
    /// * `HostError::KeyLengthOverflow` if the key size exceeds the configured limit.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn storage_read(&mut self, src_key_ptr: u64, dest_register_id: u64) -> VMLogicResult<u32> {
        let key = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }
        let key = self.read_guest_memory_slice(&key).to_vec();

        trace!(
            target: "runtime::host::storage",
            op = "read",
            key_len = key.len(),
            dest_register_id,
            "storage_read"
        );

        if let Some(value) = logic.storage.get(&key) {
            let value_len = value.len();
            self.with_logic_mut(|logic| {
                logic.registers.set(logic.limits, dest_register_id, value)
            })?;

            debug!(
                target: "runtime::host::storage",
                op = "read",
                key_len = key.len(),
                value_len,
                dest_register_id,
                "storage_read hit"
            );

            return Ok(1);
        }

        debug!(
            target: "runtime::host::storage",
            op = "read",
            key_len = key.len(),
            dest_register_id,
            "storage_read miss"
        );

        Ok(0)
    }

    /// Removes a key-value pair from persistent storage.
    ///
    /// If the key exists, the value is copied into the specified host register before removal.
    ///
    /// # Arguments
    ///
    /// * `src_key_ptr` - A pointer to the key source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory where to place
    /// the value (if found).
    ///
    /// # Returns
    ///
    /// * Returns `1` if the key was found and removed.
    /// * Returns `0` if the key was not found.
    ///
    /// # Errors
    ///
    /// * `HostError::KeyLengthOverflow` if the key size exceeds the configured limit.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn storage_remove(
        &mut self,
        src_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let key = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        let key = self.read_guest_memory_slice(&key).to_vec();

        trace!(
            target: "runtime::host::storage",
            op = "remove",
            key_len = key.len(),
            dest_register_id,
            "storage_remove"
        );

        if let Some(value) = logic.storage.get(&key) {
            let value_len = value.len();
            self.with_logic_mut(|logic| {
                let _ignored = logic.storage.remove(&key);
                logic.registers.set(logic.limits, dest_register_id, value)
            })?;

            debug!(
                target: "runtime::host::storage",
                op = "remove",
                key_len = key.len(),
                removed_value_len = value_len,
                dest_register_id,
                "storage_remove removed"
            );

            return Ok(1);
        }

        debug!(
            target: "runtime::host::storage",
            op = "remove",
            key_len = key.len(),
            dest_register_id,
            "storage_remove miss"
        );

        Ok(0)
    }

    /// Writes a key-value pair to persistent storage.
    ///
    /// If the key already exists, its value is overwritten. The old value is placed
    /// into the specified host register.
    ///
    /// # Arguments
    ///
    /// * `src_key_ptr` - A pointer to the key source-buffer in guest memory.
    /// * `src_value_ptr` - A pointer to the value source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory where to place
    /// the old value (if found).
    ///
    /// # Returns
    ///
    /// * Returns `1` if a value was evicted (i.e., the key already existed).
    /// * Returns `0` if a new key was inserted.
    ///
    /// # Errors
    ///
    /// * `HostError::KeyLengthOverflow` if the key size exceeds the limit.
    /// * `HostError::ValueLengthOverflow` if the value size exceeds the limit.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn storage_write(
        &mut self,
        src_key_ptr: u64,
        src_value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let key = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };
        let value = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_value_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        if value.len() > logic.limits.max_storage_value_size.get() {
            return Err(HostError::ValueLengthOverflow.into());
        }

        let key = self.read_guest_memory_slice(&key).to_vec();
        let value = self.read_guest_memory_slice(&value).to_vec();
        let key_len = key.len();
        let value_len = value.len();

        trace!(
            target: "runtime::host::storage",
            op = "write",
            key_len,
            value_len,
            dest_register_id,
            "storage_write"
        );

        let evicted = self.with_logic_mut(|logic| logic.storage.set(key, value));

        if let Some(evicted) = evicted {
            let evicted_len = evicted.len();
            self.with_logic_mut(|logic| {
                logic.registers.set(logic.limits, dest_register_id, evicted)
            })?;

            debug!(
                target: "runtime::host::storage",
                op = "write",
                dest_register_id,
                evicted_len,
                "storage_write evicted"
            );

            return Ok(1);
        }

        debug!(
            target: "runtime::host::storage",
            op = "write",
            dest_register_id,
            value_len,
            "storage_write new entry"
        );

        Ok(0)
    }

    /// Creates a new CRDT map and returns its identifier.
    pub fn js_crdt_map_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.crdt_map_new(dest_register_id)
    }

    /// Retrieves a value from the CRDT map.
    pub fn js_crdt_map_get(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.crdt_map_get(map_id_ptr, key_ptr, dest_register_id)
    }

    /// Inserts or replaces a value in the CRDT map.
    pub fn js_crdt_map_insert(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        value_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.crdt_map_insert(map_id_ptr, key_ptr, value_ptr, dest_register_id)
    }

    /// Removes a value from the CRDT map.
    pub fn js_crdt_map_remove(
        &mut self,
        map_id_ptr: u64,
        key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        self.crdt_map_remove(map_id_ptr, key_ptr, dest_register_id)
    }

    /// Checks whether a key exists in the CRDT map.
    pub fn js_crdt_map_contains(&mut self, map_id_ptr: u64, key_ptr: u64) -> VMLogicResult<i32> {
        self.crdt_map_contains(map_id_ptr, key_ptr)
    }

    pub fn js_crdt_vector_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
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

    pub fn js_crdt_vector_len(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
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

    pub fn js_crdt_vector_push(
        &mut self,
        vector_id_ptr: u64,
        value_ptr: u64,
    ) -> VMLogicResult<i32> {
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

    pub fn js_crdt_vector_get(
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

    pub fn js_crdt_vector_pop(
        &mut self,
        vector_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
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

    pub fn js_crdt_set_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
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

    pub fn js_crdt_set_insert(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
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
            Ok(inserted) => match save_js_set_instance(&mut set) {
                Ok(()) => Ok(i32::from(inserted)),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    pub fn js_crdt_set_contains(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
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

    pub fn js_crdt_set_remove(&mut self, set_id_ptr: u64, value_ptr: u64) -> VMLogicResult<i32> {
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
                if removed {
                    if let Err(message) = save_js_set_instance(&mut set) {
                        return self.write_error_message(0, message);
                    }
                }
                Ok(i32::from(removed))
            }
            Err(err) => self.write_error_message(0, err),
        }
    }

    pub fn js_crdt_set_len(
        &mut self,
        set_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
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

    pub fn js_crdt_set_clear(&mut self, set_id_ptr: u64) -> VMLogicResult<i32> {
        let set_id = match self.read_map_id(set_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut set = match load_js_set_instance(set_id) {
            Ok(set) => set,
            Err(message) => return self.write_error_message(0, message),
        };

        match set.clear() {
            Ok(()) => match save_js_set_instance(&mut set) {
                Ok(()) => Ok(1),
                Err(message) => self.write_error_message(0, message),
            },
            Err(err) => self.write_error_message(0, err),
        }
    }

    pub fn js_crdt_lww_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| -> Result<JsLwwRegister, String> {
            let mut register = JsLwwRegister::new();
            save_js_lww_register_instance(&mut register)?;
            Ok(register)
        }));

        match outcome {
            Ok(Ok(register)) => {
                self.write_register_bytes(dest_register_id, register.id().as_bytes())?;
                Ok(0)
            }
            Ok(Err(err)) => self.write_error_message(dest_register_id, err),
            Err(payload) => {
                self.write_error_message(dest_register_id, panic_payload_to_string(payload))
            }
        }
    }

    pub fn js_crdt_lww_set(
        &mut self,
        register_id_ptr: u64,
        value_ptr: u64,
        has_value: u32,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(0, message),
        };

        let mut register = match load_js_lww_register_instance(register_id) {
            Ok(register) => register,
            Err(message) => return self.write_error_message(0, message),
        };

        if has_value != 0 {
            let value = self.read_buffer(value_ptr)?;
            register.set(Some(&value));
        } else {
            register.set(None);
        }

        match save_js_lww_register_instance(&mut register) {
            Ok(()) => Ok(1),
            Err(message) => self.write_error_message(0, message),
        }
    }

    pub fn js_crdt_lww_get(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let register = match load_js_lww_register_instance(register_id) {
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

    pub fn js_crdt_lww_timestamp(
        &mut self,
        register_id_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<i32> {
        let register_id = match self.read_map_id(register_id_ptr)? {
            Ok(id) => id,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let register = match load_js_lww_register_instance(register_id) {
            Ok(register) => register,
            Err(message) => return self.write_error_message(dest_register_id, message),
        };

        let timestamp = register.timestamp();
        let time_bytes = timestamp.get_time().as_u64().to_le_bytes();
        let id_value: u128 = (*timestamp.get_id()).into();
        let id_bytes = id_value.to_le_bytes();

        let mut encoded = [0u8; 24];
        encoded[..8].copy_from_slice(&time_bytes);
        encoded[8..].copy_from_slice(&id_bytes);

        self.write_register_bytes(dest_register_id, &encoded)?;
        Ok(1)
    }

    pub fn js_crdt_counter_new(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
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

    pub fn js_crdt_counter_increment(&mut self, counter_id_ptr: u64) -> VMLogicResult<i32> {
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

    pub fn js_crdt_counter_value(
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
            Ok(total) => {
                self.write_register_bytes(dest_register_id, &total.to_le_bytes())?;
                Ok(1)
            }
            Err(err) => self.write_error_message(dest_register_id, err),
        }
    }

    pub fn js_crdt_counter_get_executor_count(
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
        Ok(Some(map)) => Ok(map),
        Ok(None) => Err("map not found".to_owned()),
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
        Ok(Some(vector)) => Ok(vector),
        Ok(None) => Err("vector not found".to_owned()),
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
        Ok(Some(set)) => Ok(set),
        Ok(None) => Err("set not found".to_owned()),
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
        Ok(Some(register)) => Ok(register),
        Ok(None) => Err("register not found".to_owned()),
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

fn load_js_counter_instance(id: Id) -> Result<JsCounter, String> {
    match JsCounter::load(id) {
        Ok(Some(counter)) => Ok(counter),
        Ok(None) => Err("counter not found".to_owned()),
        Err(err) => Err(err.to_string()),
    }
}

fn save_js_counter_instance(counter: &mut JsCounter) -> Result<(), String> {
    match counter.save() {
        Ok(_) => Ok(()),
        Err(StorageError::CannotCreateOrphan(_)) => {
            ensure_root_index_internal().map_err(|err| err.to_string())?;
            match Interface::<MainStorage>::add_child_to(Id::root(), counter) {
                Ok(_) => Ok(()),
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

#[cfg(test)]
mod tests {
    use crate::logic::{
        tests::{prepare_guest_buf_descriptor, setup_vm, write_str, SimpleMockStorage},
        Cow, VMContext, VMLimits, VMLogic, DIGEST_SIZE,
    };
    use wasmer::{AsStoreMut, Store};

    /// Tests the basic `storage_write` and `storage_read` host functions.
    #[test]
    fn test_storage_write_read() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "key";
        let key_ptr = 200u64;
        // Guest: write `key` to its memory.
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 10u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let value = "value";
        let value_ptr = 300u64;
        // Guest: write `value` to its memory.
        write_str(&host, value_ptr, value);
        let value_buf_ptr = 32u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;
        // Guest: as host to write key-value pair to host storage.
        let res = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap();
        // Guest: verify the storage writing was successful.
        assert_eq!(res, 0);

        // Guest: ask the host to read from it's storage with a key located at `key_buf_ptr` and
        // put the result into `register_id`.
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        // Ensure, the storage read was successful
        assert_eq!(res, 1);
        // Verify that the register length has the proper size
        assert_eq!(host.register_len(register_id).unwrap(), value.len() as u64);

        // Guest: ask the host to read the register and verify that the register has the proper
        // content after the `storage_read()` successfully exectued.
        let buf_ptr = 400u64;
        let data_output_ptr = 500u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, value.len() as u64);

        // Guest: read the register from the host into `buf_ptr`.
        let res = host.read_register(register_id, buf_ptr).unwrap();
        // Guest: assert the host successfully wrote the data from its register to our `buf_ptr`.
        assert_eq!(res, 1);

        let mut mem_buffer = vec![0u8; value.len()];
        // Host: perform a priveleged read of the contents of guest's memory to verify it
        // matches the `value`.
        host.borrow_memory()
            .read(data_output_ptr, &mut mem_buffer)
            .unwrap();
        let mem_buffer_str = std::str::from_utf8(&mem_buffer).unwrap();
        // Verify that the value from the register, after the successfull `storage_read()`
        // operation matches the same `value` when we initially wrote to the storage.
        assert_eq!(mem_buffer_str, value);
    }

    /// Tests the `storage_remove()` host function.
    #[test]
    fn test_storage_remove() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "to_remove";
        let value = "old_value";
        // Manually write into host storage for simplicity reasons.
        let _unused = host.with_logic_mut(|logic| {
            logic
                .storage
                .set(key.as_bytes().to_vec(), value.as_bytes().to_vec())
        });

        let key_ptr = 100u64;
        // Guest: write key to its memory
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let register_id = 1u64;
        // Guest: ask host to remove from storage the value with the given key.
        let res = host.storage_remove(key_buf_ptr, register_id).unwrap();
        // Verify the storage removal was successful.
        assert_eq!(res, 1);
        // Verify the storage doesn't have a specified key anymore.
        assert_eq!(
            host.borrow_logic().storage.has(&key.as_bytes().to_vec()),
            false
        );
        // Verify the removed value was put into the host register.
        assert_eq!(
            host.borrow_logic().registers.get(register_id).unwrap(),
            value.as_bytes()
        );

        // Verify the host register contains the removed value.
        let buf_ptr = 200u64;
        let data_output_ptr = 300u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, value.len() as u64);

        // Guest: read the register from the host into `buf_ptr`.
        let res = host.read_register(register_id, buf_ptr).unwrap();
        // Guest: assert the host successfully wrote the data from its register to our `buf_ptr`.
        assert_eq!(res, 1);

        let mut mem_buffer = vec![0u8; value.len()];
        // Host: perform a priveleged read of the contents of guest's memory to verify it
        // matches the `value`.
        host.borrow_memory()
            .read(data_output_ptr, &mut mem_buffer)
            .unwrap();
        assert_eq!(std::str::from_utf8(&mem_buffer).unwrap(), value);
    }
}
