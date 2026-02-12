use crate::{
    errors::HostError,
    logic::{sys, VMHostFunctions, VMLogicResult},
};
use tracing::{debug, trace};

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
        let key = self.read_guest_memory_slice(&key)?.to_vec();

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

        let key = self.read_guest_memory_slice(&key)?.to_vec();

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

        let key = self.read_guest_memory_slice(&key)?.to_vec();
        let value = self.read_guest_memory_slice(&value)?.to_vec();
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

    // ==================== Private Storage Functions ====================
    // These operate on node-local storage that is NOT synchronized.

    /// Reads a value from private (node-local) storage.
    ///
    /// Private storage is NOT synchronized across nodes - it remains local to this node only.
    ///
    /// # Arguments
    ///
    /// * `src_key_ptr` - A pointer to the key source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory.
    ///
    /// # Returns
    ///
    /// * Returns `1` if the key was found and the value was recorded into the register.
    /// * Returns `0` if the key was not found or private storage is not available.
    pub fn private_storage_read(
        &mut self,
        src_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let key = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }
        let key = self.read_guest_memory_slice(&key)?.to_vec();

        trace!(
            target: "runtime::host::private_storage",
            op = "read",
            key_len = key.len(),
            dest_register_id,
            "private_storage_read"
        );

        // Access private storage
        let logic = self.borrow_logic();
        let Some(ref private_storage) = logic.private_storage else {
            debug!(
                target: "runtime::host::private_storage",
                "private_storage_read: no private storage available"
            );
            return Ok(0);
        };

        if let Some(value) = private_storage.get(&key) {
            let value_len = value.len();
            self.with_logic_mut(|logic| {
                logic.registers.set(logic.limits, dest_register_id, value)
            })?;

            debug!(
                target: "runtime::host::private_storage",
                op = "read",
                key_len = key.len(),
                value_len,
                dest_register_id,
                "private_storage_read hit"
            );

            return Ok(1);
        }

        debug!(
            target: "runtime::host::private_storage",
            op = "read",
            key_len = key.len(),
            dest_register_id,
            "private_storage_read miss"
        );

        Ok(0)
    }

    /// Removes a key-value pair from private (node-local) storage.
    ///
    /// Private storage is NOT synchronized across nodes - it remains local to this node only.
    ///
    /// # Arguments
    ///
    /// * `src_key_ptr` - A pointer to the key source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory.
    ///
    /// # Returns
    ///
    /// * Returns `1` if the key was found and removed.
    /// * Returns `0` if the key was not found or private storage is not available.
    pub fn private_storage_remove(
        &mut self,
        src_key_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let key = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_key_ptr)? };

        let logic = self.borrow_logic();

        if key.len() > logic.limits.max_storage_key_size.get() {
            return Err(HostError::KeyLengthOverflow.into());
        }

        let key = self.read_guest_memory_slice(&key)?.to_vec();

        trace!(
            target: "runtime::host::private_storage",
            op = "remove",
            key_len = key.len(),
            dest_register_id,
            "private_storage_remove"
        );

        // Access private storage
        let value = self.with_logic_mut(|logic| {
            let Some(ref mut private_storage) = logic.private_storage else {
                return None;
            };
            private_storage.remove(&key)
        });

        if let Some(value) = value {
            let value_len = value.len();
            self.with_logic_mut(|logic| {
                logic.registers.set(logic.limits, dest_register_id, value)
            })?;

            debug!(
                target: "runtime::host::private_storage",
                op = "remove",
                key_len = key.len(),
                removed_value_len = value_len,
                dest_register_id,
                "private_storage_remove removed"
            );

            return Ok(1);
        }

        debug!(
            target: "runtime::host::private_storage",
            op = "remove",
            key_len = key.len(),
            dest_register_id,
            "private_storage_remove miss"
        );

        Ok(0)
    }

    /// Writes a key-value pair to private (node-local) storage.
    ///
    /// Private storage is NOT synchronized across nodes - it remains local to this node only.
    ///
    /// # Arguments
    ///
    /// * `src_key_ptr` - A pointer to the key source-buffer in guest memory.
    /// * `src_value_ptr` - A pointer to the value source-buffer in guest memory.
    ///
    /// # Returns
    ///
    /// * Returns `1` if the write was successful.
    /// * Returns `0` if private storage is not available.
    pub fn private_storage_write(
        &mut self,
        src_key_ptr: u64,
        src_value_ptr: u64,
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

        let key = self.read_guest_memory_slice(&key)?.to_vec();
        let value = self.read_guest_memory_slice(&value)?.to_vec();
        let key_len = key.len();
        let value_len = value.len();

        trace!(
            target: "runtime::host::private_storage",
            op = "write",
            key_len,
            value_len,
            "private_storage_write"
        );

        let written = self.with_logic_mut(|logic| {
            let Some(ref mut private_storage) = logic.private_storage else {
                return false;
            };
            let _evicted = private_storage.set(key, value);
            true
        });

        if written {
            debug!(
                target: "runtime::host::private_storage",
                op = "write",
                key_len,
                value_len,
                "private_storage_write success"
            );
            return Ok(1);
        }

        debug!(
            target: "runtime::host::private_storage",
            "private_storage_write: no private storage available"
        );

        Ok(0)
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

    /// Tests that `storage_read` returns 0 when the key is not found.
    #[test]
    fn test_storage_read_key_not_found() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "non_existent_key";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let register_id = 1u64;
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();

        // Should return 0 indicating key not found.
        assert_eq!(res, 0);
    }

    /// Tests that `storage_remove` returns 0 when the key is not found.
    #[test]
    fn test_storage_remove_key_not_found() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "non_existent_key";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let register_id = 1u64;
        let res = host.storage_remove(key_buf_ptr, register_id).unwrap();

        // Should return 0 indicating key not found.
        assert_eq!(res, 0);
    }

    /// Tests that `storage_read` fails when key length exceeds the limit.
    #[test]
    fn test_storage_read_key_too_long() {
        let mut storage = SimpleMockStorage::new();
        let mut limits = VMLimits::default();
        // Set a small key size limit for testing.
        limits.max_storage_key_size = std::num::NonZeroU64::new(10).unwrap();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Create a key that exceeds the limit.
        let key = "this_key_is_way_too_long";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let register_id = 1u64;
        let err = host.storage_read(key_buf_ptr, register_id).unwrap_err();

        assert!(matches!(
            err,
            crate::logic::VMLogicError::HostError(crate::errors::HostError::KeyLengthOverflow)
        ));
    }

    /// Tests that `storage_write` fails when key length exceeds the limit.
    #[test]
    fn test_storage_write_key_too_long() {
        let mut storage = SimpleMockStorage::new();
        let mut limits = VMLimits::default();
        // Set a small key size limit for testing.
        limits.max_storage_key_size = std::num::NonZeroU64::new(10).unwrap();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Create a key that exceeds the limit.
        let key = "this_key_is_way_too_long";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let value = "value";
        let value_ptr = 200u64;
        write_str(&host, value_ptr, value);
        let value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;
        let err = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap_err();

        assert!(matches!(
            err,
            crate::logic::VMLogicError::HostError(crate::errors::HostError::KeyLengthOverflow)
        ));
    }

    /// Tests that `storage_write` fails when value length exceeds the limit.
    #[test]
    fn test_storage_write_value_too_long() {
        let mut storage = SimpleMockStorage::new();
        let mut limits = VMLimits::default();
        // Set a small value size limit for testing.
        limits.max_storage_value_size = std::num::NonZeroU64::new(10).unwrap();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "key";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        // Create a value that exceeds the limit.
        let value = "this_value_is_way_too_long_for_the_limit";
        let value_ptr = 200u64;
        write_str(&host, value_ptr, value);
        let value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;
        let err = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap_err();

        assert!(matches!(
            err,
            crate::logic::VMLogicError::HostError(crate::errors::HostError::ValueLengthOverflow)
        ));
    }

    /// Tests that `storage_remove` fails when key length exceeds the limit.
    #[test]
    fn test_storage_remove_key_too_long() {
        let mut storage = SimpleMockStorage::new();
        let mut limits = VMLimits::default();
        // Set a small key size limit for testing.
        limits.max_storage_key_size = std::num::NonZeroU64::new(10).unwrap();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Create a key that exceeds the limit.
        let key = "this_key_is_way_too_long";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let register_id = 1u64;
        let err = host.storage_remove(key_buf_ptr, register_id).unwrap_err();

        assert!(matches!(
            err,
            crate::logic::VMLogicError::HostError(crate::errors::HostError::KeyLengthOverflow)
        ));
    }

    /// Tests `storage_write` overwriting an existing key returns the old value.
    #[test]
    fn test_storage_write_overwrite_returns_old_value() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "key";
        let old_value = "old_value";
        let new_value = "new_value";

        // First write.
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        let old_value_ptr = 200u64;
        write_str(&host, old_value_ptr, old_value);
        let old_value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(
            &host,
            old_value_buf_ptr,
            old_value_ptr,
            old_value.len() as u64,
        );

        let register_id = 1u64;
        let res = host
            .storage_write(key_buf_ptr, old_value_buf_ptr, register_id)
            .unwrap();
        // First write should return 0 (no eviction).
        assert_eq!(res, 0);

        // Second write (overwrite).
        let new_value_ptr = 300u64;
        write_str(&host, new_value_ptr, new_value);
        let new_value_buf_ptr = 48u64;
        prepare_guest_buf_descriptor(
            &host,
            new_value_buf_ptr,
            new_value_ptr,
            new_value.len() as u64,
        );

        let res = host
            .storage_write(key_buf_ptr, new_value_buf_ptr, register_id)
            .unwrap();
        // Second write should return 1 (eviction occurred).
        assert_eq!(res, 1);

        // Verify the evicted (old) value is in the register.
        let evicted_value = host.borrow_logic().registers.get(register_id).unwrap();
        assert_eq!(evicted_value, old_value.as_bytes());
    }

    /// Tests storage operations with empty key.
    #[test]
    fn test_storage_operations_with_empty_key() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Empty key.
        let key_ptr = 100u64;
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, 0);

        let value = "value_for_empty_key";
        let value_ptr = 200u64;
        write_str(&host, value_ptr, value);
        let value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;

        // Write with empty key.
        let res = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap();
        assert_eq!(res, 0);

        // Read with empty key.
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        assert_eq!(res, 1);
        assert_eq!(
            host.borrow_logic().registers.get(register_id).unwrap(),
            value.as_bytes()
        );

        // Remove with empty key.
        let res = host.storage_remove(key_buf_ptr, register_id).unwrap();
        assert_eq!(res, 1);

        // Verify empty key is removed.
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        assert_eq!(res, 0);
    }

    /// Tests storage operations with empty value.
    #[test]
    fn test_storage_operations_with_empty_value() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let key = "key_with_empty_value";
        let key_ptr = 100u64;
        write_str(&host, key_ptr, key);
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        // Empty value.
        let value_ptr = 200u64;
        let value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, 0);

        let register_id = 1u64;

        // Write with empty value.
        let res = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap();
        assert_eq!(res, 0);

        // Read should find the key with empty value.
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        assert_eq!(res, 1);
        assert_eq!(
            host.borrow_logic().registers.get(register_id).unwrap(),
            &[] as &[u8]
        );
    }

    /// Tests storage with binary (non-UTF8) key and value.
    #[test]
    fn test_storage_with_binary_data() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Binary key with non-UTF8 bytes.
        let key: Vec<u8> = vec![0x00, 0xFF, 0x80, 0x7F];
        let key_ptr = 100u64;
        host.borrow_memory().write(key_ptr, &key).unwrap();
        let key_buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

        // Binary value with non-UTF8 bytes.
        let value: Vec<u8> = vec![0x01, 0x02, 0xFE, 0xFD];
        let value_ptr = 200u64;
        host.borrow_memory().write(value_ptr, &value).unwrap();
        let value_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

        let register_id = 1u64;

        // Write binary data.
        let res = host
            .storage_write(key_buf_ptr, value_buf_ptr, register_id)
            .unwrap();
        assert_eq!(res, 0);

        // Read binary data.
        let res = host.storage_read(key_buf_ptr, register_id).unwrap();
        assert_eq!(res, 1);
        assert_eq!(
            host.borrow_logic().registers.get(register_id).unwrap(),
            &value
        );
    }

    /// Tests multiple consecutive storage operations.
    #[test]
    fn test_multiple_storage_operations() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Write multiple keys.
        for i in 0..5 {
            let key = format!("key_{}", i);
            let value = format!("value_{}", i);

            let key_ptr = (100 + i * 100) as u64;
            write_str(&host, key_ptr, &key);
            let key_buf_ptr = (16 + i * 32) as u64;
            prepare_guest_buf_descriptor(&host, key_buf_ptr, key_ptr, key.len() as u64);

            let value_ptr = (500 + i * 100) as u64;
            write_str(&host, value_ptr, &value);
            let value_buf_ptr = (48 + i * 32) as u64;
            prepare_guest_buf_descriptor(&host, value_buf_ptr, value_ptr, value.len() as u64);

            let register_id = (i + 1) as u64;
            host.storage_write(key_buf_ptr, value_buf_ptr, register_id)
                .unwrap();
        }

        // Read all keys and verify values.
        for i in 0..5 {
            let key = format!("key_{}", i);
            let expected_value = format!("value_{}", i);

            let key_ptr = (100 + i * 100) as u64;
            let key_buf_ptr = (16 + i * 32) as u64;
            // Reuse the already prepared descriptors.

            let register_id = (i + 10) as u64;
            let res = host.storage_read(key_buf_ptr, register_id).unwrap();
            assert_eq!(res, 1);
            assert_eq!(
                host.borrow_logic().registers.get(register_id).unwrap(),
                expected_value.as_bytes()
            );
        }
    }
}
