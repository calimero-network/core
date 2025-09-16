use serde::Serialize;

use crate::{
    errors::{HostError, Location, PanicContext},
    logic::{sys, VMHostFunctions, VMLogicResult, DIGEST_SIZE},
};

/// Represents a structured event emitted during the execution.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Event {
    /// A string identifying the type or category of the event.
    pub kind: String,
    /// The binary data payload associated with the event.
    pub data: Vec<u8>,
}

impl VMHostFunctions<'_> {
    /// Host function to handle a simple panic from the guest.
    ///
    /// This function is called when the guest code panics without a message. It captures
    /// the source location (file, line, column) of the panic and terminates the execution.
    ///
    /// # Arguments
    ///
    /// * `src_location_ptr` - A pointer in guest memory to a `sys::Location` struct,
    ///   containing file, line, and column information about the panic's origin.
    ///
    /// # Returns/Errors
    ///
    /// * `HostError::Panic` if the panic action was successfully executed.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn panic(&mut self, src_location_ptr: u64) -> VMLogicResult<()> {
        let location =
            unsafe { self.read_guest_memory_typed::<sys::Location<'_>>(src_location_ptr)? };

        let file = self.read_guest_memory_str(&location.file())?.to_owned();
        let line = location.line();
        let column = location.column();

        Err(HostError::Panic {
            context: PanicContext::Guest,
            message: "explicit panic".to_owned(),
            location: Location::At { file, line, column },
        }
        .into())
    }

    /// Host function to handle a panic with a UTF-8 message from the guest.
    ///
    /// This function is called when guest code panics with a message. It captures the
    /// message and source location, then terminates the execution.
    ///
    /// # Arguments
    ///
    /// * `src_panic_msg_ptr` - A pointer in guest memory to a source-buffer `sys::Buffer` containing
    /// the UTF-8 panic message.
    /// * `src_location_ptr` - A pointer in guest memory to a `sys::Location` struct for the panic's origin.
    ///
    /// # Returns/Errors
    ///
    /// * `HostError::Panic` if the panic action was successfully executed.
    /// * `HostError::BadUTF8` if reading UTF8 string from guest memory fails.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn panic_utf8(
        &mut self,
        src_panic_msg_ptr: u64,
        src_location_ptr: u64,
    ) -> VMLogicResult<()> {
        let panic_message_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_panic_msg_ptr)? };
        let location =
            unsafe { self.read_guest_memory_typed::<sys::Location<'_>>(src_location_ptr)? };

        let panic_message = self.read_guest_memory_str(&panic_message_buf)?.to_owned();
        let file = self.read_guest_memory_str(&location.file())?.to_owned();
        let line = location.line();
        let column = location.column();

        Err(HostError::Panic {
            context: PanicContext::Guest,
            message: panic_message,
            location: Location::At { file, line, column },
        }
        .into())
    }

    /// Returns the length of the data in a given register.
    ///
    /// # Arguments
    ///
    /// * `register_id` - The ID of the register to query.
    ///
    /// # Returns
    ///
    /// The length of the data in the specified register. If the register is not found,
    /// it returns `u64::MAX`.
    pub fn register_len(&self, register_id: u64) -> VMLogicResult<u64> {
        Ok(self
            .borrow_logic()
            .registers
            .get_len(register_id)
            .unwrap_or(u64::MAX))
    }

    /// Reads the data from a register into a guest memory buffer.
    ///
    /// # Arguments
    ///
    /// * `register_id` - The ID of the register to read from.
    /// * `dest_data_ptr` - A pointer in guest memory to a destination buffer `sys::BufferMut`
    /// where the data should be copied.
    ///
    /// # Returns
    ///
    /// * Returns `1` if the data was successfully read and copied.
    /// * Returns `0` if the provided guest buffer has a different length than the register's data.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidRegisterId` if the register does not exist.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn read_register(&self, register_id: u64, dest_data_ptr: u64) -> VMLogicResult<u32> {
        let dest_data =
            unsafe { self.read_guest_memory_typed::<sys::BufferMut<'_>>(dest_data_ptr)? };

        let data = self.borrow_logic().registers.get(register_id)?;

        if data.len() != usize::try_from(dest_data.len()).map_err(|_| HostError::IntegerOverflow)? {
            return Ok(0);
        }

        self.read_guest_memory_slice_mut(&dest_data)
            .copy_from_slice(data);

        Ok(1)
    }

    /// Copies the current context ID into a register.
    ///
    /// # Arguments
    ///
    /// * `dest_register_id` - The ID of the destination register.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails (e.g., exceeds limits).
    pub fn context_id(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, dest_register_id, logic.context.context_id)
        })
    }

    /// Copies the executor's public key into a register.
    ///
    /// # Arguments
    ///
    /// * `dest_register_id` - The ID of the destination register.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails (e.g., exceeds limits).
    pub fn executor_id(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic.registers.set(
                logic.limits,
                dest_register_id,
                logic.context.executor_public_key,
            )
        })
    }

    /// Copies the input data for the current execution (from context ID) into a register.
    ///
    /// # Arguments
    ///
    /// * `dest_register_id` - The ID of the destination register.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails (e.g., exceeds limits).
    pub fn input(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic
                .registers
                .set(logic.limits, dest_register_id, &*logic.context.input)
        })?;

        Ok(())
    }

    /// Sets the final return value of the execution.
    ///
    /// This function can be called by the guest to specify a successful result (`Ok`)
    /// or a custom execution error (`Err`). This value will be part of the final `Outcome`.
    ///
    /// # Arguments
    ///
    /// * `src_value_ptr` - A pointer in guest memory to a source-`sys::ValueReturn`,
    /// which is an enum indicating success or error, along with the data buffer.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn value_return(&mut self, src_value_ptr: u64) -> VMLogicResult<()> {
        let result =
            unsafe { self.read_guest_memory_typed::<sys::ValueReturn<'_>>(src_value_ptr)? };

        let result = match result {
            sys::ValueReturn::Ok(value) => Ok(self.read_guest_memory_slice(&value).to_vec()),
            sys::ValueReturn::Err(value) => Err(self.read_guest_memory_slice(&value).to_vec()),
        };

        self.with_logic_mut(|logic| logic.returns = Some(result));

        Ok(())
    }

    /// Adds a new log message (UTF-8 encoded string) to the execution log. The message is being
    /// obtained from the guest memory.
    ///
    /// # Arguments
    ///
    /// * `src_log_ptr` - A pointer in guest memory to a source-`sys::Buffer` containing the log message.
    ///
    /// # Errors
    ///
    /// * `HostError::LogsOverflow` if the maximum number of logs has been reached.
    /// * `HostError::BadUTF8` if the message is not a valid UTF-8 string.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn log_utf8(&mut self, src_log_ptr: u64) -> VMLogicResult<()> {
        let src_log_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_log_ptr)? };

        let logic = self.borrow_logic();

        if logic.logs.len()
            >= usize::try_from(logic.limits.max_logs).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::LogsOverflow.into());
        }

        let message = self.read_guest_memory_str(&src_log_buf)?.to_owned();

        self.with_logic_mut(|logic| logic.logs.push(message));

        Ok(())
    }

    /// Emits a structured event that is added to the events log.
    ///
    /// Events are recorded and included in the final execution `Outcome`.
    ///
    /// # Arguments
    ///
    /// * `src_event_ptr` - A pointer in guest memory to a `sys::Event` struct, which
    /// contains source-buffers for the event `kind` and `data`.
    ///
    /// # Errors
    ///
    /// * `HostError::EventKindSizeOverflow` if the event kind is too long.
    /// * `HostError::EventDataSizeOverflow` if the event data is too large.
    /// * `HostError::EventsOverflow` if the maximum number of events has been reached.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn emit(&mut self, src_event_ptr: u64) -> VMLogicResult<()> {
        let event = unsafe { self.read_guest_memory_typed::<sys::Event<'_>>(src_event_ptr)? };

        let kind_len = event.kind().len();
        let data_len = event.data().len();

        let logic = self.borrow_logic();

        if kind_len > logic.limits.max_event_kind_size {
            return Err(HostError::EventKindSizeOverflow.into());
        }

        if data_len > logic.limits.max_event_data_size {
            return Err(HostError::EventDataSizeOverflow.into());
        }

        if logic.events.len()
            >= usize::try_from(logic.limits.max_events).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::EventsOverflow.into());
        }

        let kind = self.read_guest_memory_str(event.kind())?.to_owned();
        let data = self.read_guest_memory_slice(event.data()).to_vec();

        self.with_logic_mut(|logic| logic.events.push(Event { kind, data }));

        Ok(())
    }

    /// Commits the execution state, providing a state root and an artifact.
    ///
    /// This function can only be called once per execution.
    ///
    /// # Arguments
    ///
    /// * `src_root_hash_ptr` - A pointer to a source-buffer in guest memory containing the 32-byte state root hash.
    /// * `src_artifact_ptr` - A pointer to a source-buffer in guest memory containing a binary artifact.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if this function is called more than once or if memory
    /// access fails for descriptor buffers.
    pub fn commit(&mut self, src_root_hash_ptr: u64, src_artifact_ptr: u64) -> VMLogicResult<()> {
        let root_hash =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_root_hash_ptr)? };
        let artifact =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_artifact_ptr)? };

        let root_hash = *self.read_guest_memory_sized::<DIGEST_SIZE>(&root_hash)?;
        let artifact = self.read_guest_memory_slice(&artifact).to_vec();

        self.with_logic_mut(|logic| {
            if logic.root_hash.is_some() {
                return Err(HostError::InvalidMemoryAccess);
            }

            logic.root_hash = Some(root_hash);
            logic.artifact = artifact;

            Ok(())
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use wasmer::{AsStoreMut, Store};

    use crate::errors::{HostError, Location};
    use crate::logic::{
        tests::{
            prepare_guest_buf_descriptor, setup_vm, write_str, SimpleMockStorage, DESCRIPTOR_SIZE,
        },
        Cow, VMContext, VMLimits, VMLogic, VMLogicError, DIGEST_SIZE,
    };

    /// Tests the `input()`, `register_len()`, `read_register()` host functions.
    #[test]
    fn test_input_and_basic_registers_api() {
        let input = vec![1u8, 2, 3];
        let input_len = input.len() as u64;
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, input.clone());

        {
            let mut host = logic.host_functions(store.as_store_mut());
            let register_id = 1u64;

            // Guest: load the context data into a host-side register.
            host.input(register_id).expect("Input call failed");
            // Guest: verify the byte length of the host-side register's data matches the input length.
            assert_eq!(host.register_len(register_id).unwrap(), input_len);

            let buf_ptr = 100u64;
            let data_output_ptr = 200u64;
            // Guest: prepare the descriptor for the destination buffer so host can write there.
            prepare_guest_buf_descriptor(&host, buf_ptr, data_output_ptr, input_len);

            // Guest: read the register from the host into `buf_ptr`.
            let res = host.read_register(register_id, buf_ptr).unwrap();
            // Guest: assert the host successfully wrote the data from its register to our `buf_ptr`.
            assert_eq!(res, 1);

            let mut mem_buffer = vec![0u8; input_len as usize];
            // Host: perform a priveleged read of the contents of guest's memory to verify it
            // matches the `input`.
            host.borrow_memory()
                .read(data_output_ptr, &mut mem_buffer)
                .unwrap();
            assert_eq!(mem_buffer, input);
        }
    }

    /// Tests the `context_id()` and `executor_id()` host functions.
    ///
    /// This test verifies that the guest can request and receive context and executor IDs.
    #[test]
    fn test_context_and_executor_id() {
        let context_id = [3u8; DIGEST_SIZE];
        let executor_id = [5u8; DIGEST_SIZE];
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let context = VMContext::new(Cow::Owned(vec![]), context_id, executor_id);
        let mut logic = VMLogic::new(&mut storage, context, &limits, None);

        let mut store = Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
        let _ = logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        let context_id_register = 1;
        // Guest: ask the host to put the context ID into host register
        // that has a value `context_id_register`.
        host.context_id(context_id_register).unwrap();
        // Very the `context_id` is correctly written into its host-side register.
        let requested_context_id = host
            .borrow_logic()
            .registers
            .get(context_id_register)
            .unwrap();
        assert_eq!(requested_context_id, context_id);

        let executor_id_register = 2;
        // Guest: ask the host to put the executor ID into host register
        // that has a value `executor_id_register`.
        host.executor_id(executor_id_register).unwrap();
        // Verify the `executor_id` is correctly written into its host-side register.
        let requested_executor_id = host
            .borrow_logic()
            .registers
            .get(executor_id_register)
            .unwrap();
        assert_eq!(requested_executor_id, executor_id);
    }

    /// Tests the `value_return()` host function for both `Ok` and `Err` variants.
    ///
    /// This test verifies the primary mechanism for a guest to finish its execution
    /// and return a final value to the host. It checks that both successful (`Ok`) and
    /// unsuccessful (`Err`) return values are correctly stored in the `VMLogic` state.
    #[test]
    fn test_value_return() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Test returning an Ok value
        let ok_value = "this is Ok value";
        let ok_value_ptr = 200u64;
        // Guest: write ok
        write_str(&host, ok_value_ptr, ok_value);

        // Write a `sys::ValueReturn::Ok` enum representation (0) to memory.
        // The value then is followed by the buffer.
        let ok_discriminant = 0u8;
        let ok_return_ptr = 32u64;
        host.borrow_memory()
            .write(ok_return_ptr, &[ok_discriminant])
            .unwrap();
        // Guest: prepare the descriptor for the buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            ok_return_ptr + 8,
            ok_value_ptr,
            ok_value.len() as u64,
        );

        // Guest: ask host to read the return value.
        host.value_return(ok_return_ptr).unwrap();
        let returned_ok_value = host.borrow_logic().returns.clone().unwrap().unwrap();
        let returned_ok_value_str = std::str::from_utf8(&returned_ok_value).unwrap();
        // Verify the returned value matches the one from the guest.
        assert_eq!(returned_ok_value_str, ok_value);

        // Test returning an Err value
        let err_value = "this is Err value";
        let err_value_ptr = 400u64;
        write_str(&host, err_value_ptr, err_value);

        // Write a `sys::ValueReturn::Ok` enum representation (1) to memory.
        // The value then is followed by the buffer.
        let err_discriminant = 1u8;
        let err_return_ptr = 64u64;
        host.borrow_memory()
            .write(err_return_ptr, &[err_discriminant])
            .unwrap();
        // Guest: prepare the descriptor for the buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            err_return_ptr + 8,
            err_value_ptr,
            err_value.len() as u64,
        );

        // Guest: ask host to read the return value.
        host.value_return(err_return_ptr).unwrap();
        let returned_err_value = host.borrow_logic().returns.clone().unwrap().unwrap_err();
        let returned_err_value_str = std::str::from_utf8(&returned_err_value).unwrap();
        // Verify the returned value matches the one from the guest.
        assert_eq!(returned_err_value_str, err_value);
    }

    /// Tests the `log_utf8()` host function for a successful log operation.
    #[test]
    fn test_log_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "test log";
        let msg_ptr = 200u64;
        // Guest: write msg to its memory.
        write_str(&host, msg_ptr, msg);

        let buf_ptr = 10u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);
        // Guest: ask the host to log the contents of `buf_ptr`'s descriptor.
        host.log_utf8(buf_ptr).expect("Log failed");

        // Guest: verify the host successfully logged the message
        assert_eq!(host.borrow_logic().logs.len(), 1);
        assert_eq!(host.borrow_logic().logs[0], "test log");
    }

    /// Tests that the `log_utf8()` host function correctly handles the log limit and properly returns
    /// an error `HostError::LogOverflow` when the logs limit is exceeded.
    #[test]
    fn test_log_utf8_overflow() {
        let mut storage = SimpleMockStorage::new();
        let mut limits = VMLimits::default();
        limits.max_logs = 5;
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "log";
        let msg_ptr = 200u64;
        // Guest: write msg to its memory.
        write_str(&host, msg_ptr, msg);
        let buf_ptr = 10u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);

        // Guest: ask the host to log for a max limit of logs
        for _ in 0..limits.max_logs {
            host.log_utf8(buf_ptr).expect("Log failed");
        }

        // Guest: verify the host successfully logged `limits.max_logs` msgs.
        assert_eq!(host.borrow_logic().logs.len(), limits.max_logs as usize);
        // Guest: do over-the limit log
        let err = host.log_utf8(buf_ptr).unwrap_err();
        // Guest: verify the host didn't log over the limit and returned an error.
        assert_eq!(host.borrow_logic().logs.len(), limits.max_logs as usize);
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::LogsOverflow)
        ));
    }

    /// Tests that the `log_utf8()` host function correctly handles the bad UTF8 and properly returns
    /// an error `HostError::BadUTF8` when the incorrect string is provided (the failure occurs
    /// because of the verification happening inside the private `read_guest_memory_str` function).
    #[test]
    fn test_log_utf8_with_bad_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Prepare invalid UTF-8 bytes in guest memory.
        let invalid_utf8: &[u8] = &[0, 159, 146, 150];
        let data_ptr = 200u64;
        host.borrow_memory().write(data_ptr, invalid_utf8).unwrap();

        let buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, invalid_utf8.len() as u64);

        // `log_utf8` calls `read_guest_memory_str` internally. We expect it to fail.
        let err = host.log_utf8(buf_ptr).unwrap_err();
        assert!(matches!(err, VMLogicError::HostError(HostError::BadUTF8)));
    }

    /// Tests the `panic()` host function (without a custom message).
    #[test]
    fn test_panic() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let expected_file_name = "simple_panic.rs";
        let file_ptr = 400u64;
        // Guest: write file name to its memory.
        write_str(&host, file_ptr, expected_file_name);

        let loc_data_ptr = 300u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(
            &host,
            loc_data_ptr,
            file_ptr,
            expected_file_name.len() as u64,
        );

        let expected_line: u32 = 10;
        let expected_column: u32 = 5;
        let u32_size: u64 = (u32::BITS / 8).into();
        // Host: perform a priveleged write to the contents of guest's memory with a line and column
        // of the expected panic message. We write the `line` after the descriptor, and the `column` -
        // after the `line`.
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64,
                &expected_line.to_le_bytes(),
            )
            .unwrap();
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64 + u32_size,
                &expected_column.to_le_bytes(),
            )
            .unwrap();

        // Guest: ask the host to panic with the given location data.
        let err = host.panic(loc_data_ptr).unwrap_err();
        // Guest: assert the host panics with a "explicit panic" message, and `Location` (consisting
        // of file name, line, and column).
        match err {
            VMLogicError::HostError(HostError::Panic {
                message, location, ..
            }) => {
                assert_eq!(message, "explicit panic");
                match location {
                    Location::At { file, line, column } => {
                        assert_eq!(file, expected_file_name);
                        assert_eq!(line, expected_line);
                        assert_eq!(column, expected_column);
                    }
                    _ => panic!("Unexpected location variant"),
                }
            }
            _ => panic!("Unexpected error variant"),
        }
    }

    /// Tests the `panic_utf8()` host function.
    #[test]
    fn test_panic_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let expected_msg = "panic message";
        let msg_ptr = 200u64;
        // Guest: write msg to its memory.
        write_str(&host, msg_ptr, expected_msg);
        let msg_buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, msg_buf_ptr, msg_ptr, expected_msg.len() as u64);

        let expected_file_name = "file.rs";
        let file_ptr = 400u64;
        // Guest: write file name to its memory.
        write_str(&host, file_ptr, expected_file_name);

        let loc_data_ptr = 300u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(
            &host,
            loc_data_ptr,
            file_ptr,
            expected_file_name.len() as u64,
        );

        let expected_line: u32 = 10;
        let expected_column: u32 = 5;
        let u32_size: u64 = (u32::BITS / 8).into();
        // Host: perform a priveleged write to the contents of guest's memory with a line and column
        // of the expected panic message. We write the `line` after the descriptor, and the `column` -
        // after the `line`.
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64,
                &expected_line.to_le_bytes(),
            )
            .unwrap();
        host.borrow_memory()
            .write(
                loc_data_ptr + DESCRIPTOR_SIZE as u64 + u32_size,
                &expected_column.to_le_bytes(),
            )
            .unwrap();

        // Guest: ask the host to panic with the given msg and location.
        let err = host.panic_utf8(msg_buf_ptr, loc_data_ptr).unwrap_err();
        // Guest: assert the host panics with a specified panic message, and `Location` (consisting
        // of file name, line, and column).
        match err {
            VMLogicError::HostError(HostError::Panic {
                message, location, ..
            }) => {
                assert_eq!(message, expected_msg);
                match location {
                    Location::At { file, line, column } => {
                        assert_eq!(file, expected_file_name);
                        assert_eq!(line, expected_line);
                        assert_eq!(column, expected_column);
                    }
                    _ => panic!("Unexpected location variant"),
                }
            }
            _ => panic!("Unexpected error variant"),
        }
    }

    /// Tests the `emit()` host function for event creation and events overflow.
    #[test]
    fn test_emit_and_events_overflow() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Prepare a valid event
        let kind = "my-event";
        let data = vec![1, 2, 3];
        let kind_ptr = 200u64;
        let data_ptr = 300u64;
        // Guest: write msg to its memory.
        write_str(&host, kind_ptr, kind);
        host.borrow_memory().write(data_ptr, &data).unwrap();

        // Prepare the sys::Event struct in memory.
        let event_struct_ptr = 48u64;
        let kind_buf_ptr = event_struct_ptr;
        let data_buf_ptr = event_struct_ptr + DESCRIPTOR_SIZE as u64;
        prepare_guest_buf_descriptor(&host, kind_buf_ptr, kind_ptr, kind.len() as u64);
        prepare_guest_buf_descriptor(&host, data_buf_ptr, data_ptr, data.len() as u64);

        // Guest: ask host to emit the event located at `event_struct_ptr`.
        host.emit(event_struct_ptr).unwrap();
        // Test successful event emission
        assert_eq!(host.borrow_logic().events.len(), 1);
        assert_eq!(host.borrow_logic().events[0].kind, kind);
        assert_eq!(host.borrow_logic().events[0].data, data);

        // Test events overflow
        for _ in 1..limits.max_events {
            host.emit(event_struct_ptr).unwrap();
        }
        assert_eq!(host.borrow_logic().events.len() as u64, limits.max_events);
        // Guest: ask the host to do over the limit event emission.
        let err = host.emit(event_struct_ptr).unwrap_err();
        // Guest: verify the host didn't emit over the limit and returned an error.
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::EventsOverflow)
        ));
    }

    /// Tests the `commit()` host function.
    #[test]
    fn test_commit() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let root_hash = [1u8; DIGEST_SIZE];
        let artifact = vec![1, 2, 3];
        let root_hash_ptr = 200u64;
        let artifact_ptr = 300u64;
        host.borrow_memory()
            .write(root_hash_ptr, &root_hash)
            .unwrap();
        host.borrow_memory().write(artifact_ptr, &artifact).unwrap();

        let root_hash_buf_ptr = 16u64;
        let artifact_buf_ptr = 32u64;
        // Guest: prepare the descriptor for the root_hash and artifact buffers so host can access them.
        prepare_guest_buf_descriptor(
            &host,
            root_hash_buf_ptr,
            root_hash_ptr,
            root_hash.len() as u64,
        );
        prepare_guest_buf_descriptor(&host, artifact_buf_ptr, artifact_ptr, artifact.len() as u64);

        // Guest: ask host to commit with the given root hash and artifact.
        host.commit(root_hash_buf_ptr, artifact_buf_ptr).unwrap();
        // Verify the host successfully stored the root hash and artifact in the `VMLogic` state.
        assert_eq!(host.borrow_logic().root_hash, Some(root_hash));
        assert_eq!(host.borrow_logic().artifact, artifact);
    }
}
