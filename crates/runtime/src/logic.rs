#![allow(single_use_lifetimes, unused_lifetimes, reason = "False positive")]
#![allow(clippy::mem_forget, reason = "Safe for now")]

use core::mem::MaybeUninit;
use core::num::NonZeroU64;
use core::{fmt, slice};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::vec;

use calimero_node_primitives::client::NodeClient;
use calimero_sys as sys;
use ouroboros::self_referencing;
use serde::Serialize;

use crate::constants::{DIGEST_SIZE, ONE_GIB, ONE_KIB, ONE_MIB};
use crate::constraint::{Constrained, MaxU64};
use crate::errors::{FunctionCallError, HostError, Location, PanicContext};
use crate::store::Storage;
use crate::Constraint;

mod errors;
mod host_functions;
mod imports;
mod registers;

pub use errors::VMLogicError;
pub use host_functions::*;
use registers::Registers;

/// A specialized `Result` type for VMLogic operations.
pub type VMLogicResult<T, E = VMLogicError> = Result<T, E>;

/// Encapsulates the context for a single VM execution.
///
/// This struct holds all the necessary information about the current execution environment,
/// such as the input data, the context ID, and the executor's public key.
#[derive(Debug)]
#[non_exhaustive]
pub struct VMContext<'a> {
    /// The input data for the context.
    pub input: Cow<'a, [u8]>,
    /// The unique ID for the current execution context.
    pub context_id: [u8; DIGEST_SIZE],
    /// The public key of the entity executing the function call/transaction.
    pub executor_public_key: [u8; DIGEST_SIZE],
}

impl<'a> VMContext<'a> {
    /// Creates a new `VMContext`.
    ///
    /// # Arguments
    ///
    /// * `input` - The input data for the context.
    /// * `context_id` - The unique ID for the execution context.
    /// * `executor_public_key` - The public key of the executor.
    #[must_use]
    pub const fn new(
        input: Cow<'a, [u8]>,
        context_id: [u8; DIGEST_SIZE],
        executor_public_key: [u8; DIGEST_SIZE],
    ) -> Self {
        Self {
            input,
            context_id,
            executor_public_key,
        }
    }
}

/// Defines the resource limits for a VM instance.
///
/// This struct is used to configure constraints on various VM operations to prevent
/// excessive resource consumption.
#[derive(Debug, Clone, Copy)]
pub struct VMLimits {
    /// The maximum number of memory pages allowed.
    pub max_memory_pages: u32,
    /// The maximum stack size in bytes.
    pub max_stack_size: usize,
    /// The maximum number of registers that can be used.
    pub max_registers: u64,
    /// The maximum size of a single register's data in bytes.
    /// constrained to be less than u64::MAX
    /// because register_len returns u64::MAX if the register is not found
    pub max_register_size: Constrained<u64, MaxU64<{ u64::MAX - 1 }>>,
    /// The total capacity across all registers in bytes.
    pub max_registers_capacity: u64, // todo! must not be less than max_register_size
    /// The maximum number of log entries that can be created.
    pub max_logs: u64,
    /// The maximum size of a single log message in bytes.
    pub max_log_size: u64,
    /// The maximum number of events that can be emitted.
    pub max_events: u64,
    /// The maximum size of an event's "kind" string in bytes.
    pub max_event_kind_size: u64,
    /// The maximum size of an event's data payload in bytes.
    pub max_event_data_size: u64,
    /// The maximum size of a storage key in bytes.
    pub max_storage_key_size: NonZeroU64,
    /// The maximum size of a storage value in bytes.
    pub max_storage_value_size: NonZeroU64,
    /// The maximum number of blob handles that can exist.
    pub max_blob_handles: u64,
    /// The maximum size of a single chunk when writing to or reading from a blob.
    pub max_blob_chunk_size: u64,
}

impl Default for VMLimits {
    fn default() -> Self {
        #[inline(always)]
        fn is_valid<T, E: fmt::Debug>(t: Result<T, E>) -> T {
            t.expect("is valid")
        }

        Self {
            max_memory_pages: ONE_KIB,                                          // 1 KiB
            max_stack_size: 200 * ONE_KIB as usize,                             // 200 KiB
            max_registers: 100,                                                 //
            max_register_size: is_valid((100 * ONE_MIB as u64).validate()),     // 100 MiB
            max_registers_capacity: ONE_GIB as u64,                             // 1 GiB
            max_logs: 100,                                                      //
            max_log_size: 16 * ONE_KIB as u64,                                  // 16 KiB
            max_events: 100,                                                    //
            max_event_kind_size: 100,                                           //
            max_event_data_size: 16 * ONE_KIB as u64,                           // 16 KiB
            max_storage_key_size: is_valid((ONE_MIB as u64).try_into()),        // 1 MiB
            max_storage_value_size: is_valid((10 * ONE_MIB as u64).try_into()), // 10 MiB
            max_blob_handles: 100,                                              //
            max_blob_chunk_size: 10 * ONE_MIB as u64,                           // 10 MiB
        }
    }
}

/// The core logic controller for the VM.
///
/// This struct manages the state of an execution, including storage,
/// memory, registers, and the results of the execution (logs, events, return values).
#[expect(
    missing_debug_implementations,
    reason = "storage and node_client can't impl Debug"
)]
pub struct VMLogic<'a> {
    /// A mutable reference to the storage.
    storage: &'a mut dyn Storage,
    /// The Wasmer memory instance associated with the guest module.
    memory: Option<wasmer::Memory>,
    /// The execution context for the current call.
    context: VMContext<'a>,
    /// The VM resource limits applied to this execution.
    limits: &'a VMLimits,
    /// A collection of registers for temporary data exchange between host and guest.
    registers: Registers,
    /// The optional final result of the execution, which can be a success (`Ok`) or an error (`Err`).
    returns: Option<VMLogicResult<Vec<u8>, Vec<u8>>>,
    /// A list of log messages generated during execution.
    logs: Vec<String>,
    /// A list of events emitted during execution.
    events: Vec<Event>,
    /// The root hash of the state after a successful commit.
    root_hash: Option<[u8; DIGEST_SIZE]>,
    /// A binary artifact produced by the execution.
    artifact: Vec<u8>,
    /// A map of proposals created during execution, having proposal ID as a key.
    proposals: BTreeMap<[u8; DIGEST_SIZE], Vec<u8>>,
    /// A list of approvals submitted during execution.
    approvals: Vec<[u8; DIGEST_SIZE]>,

    // Blob functionality
    /// An optional client for interacting with the node's blob storage.
    node_client: Option<NodeClient>,
    /// A map of active blob handles, having blob's file descriptor as a key.
    blob_handles: HashMap<u64, BlobHandle>,
    /// The next available file descriptor for a new blob handle.
    next_blob_fd: u64,
}

impl<'a> VMLogic<'a> {
    /// Creates a new `VMLogic` instance.
    ///
    /// # Arguments
    ///
    /// * `storage` - A mutable reference to the storage implementation.
    /// * `context` - The execution context for the VM.
    /// * `limits` - The VM resource limits to enforce.
    /// * `node_client` - An optional client for blob storage operations.
    pub fn new(
        storage: &'a mut dyn Storage,
        context: VMContext<'a>,
        limits: &'a VMLimits,
        node_client: Option<NodeClient>,
    ) -> Self {
        VMLogic {
            storage,
            memory: None,
            context,
            limits,
            registers: Registers::default(),
            returns: None,
            logs: vec![],
            events: vec![],
            root_hash: None,
            artifact: vec![],
            proposals: BTreeMap::new(),
            approvals: vec![],

            // Blob functionality
            node_client,
            blob_handles: HashMap::new(),
            next_blob_fd: 1,
        }
    }

    /// Associates a Wasmer memory instance with this `VMLogic`.
    ///
    /// This method should be called after the guest module is instantiated but before
    /// any host functions are called.
    ///
    /// # Arguments
    ///
    /// * `memory` - The `wasmer::Memory` instance from the instantiated guest module.
    pub fn with_memory(&mut self, memory: wasmer::Memory) -> &mut Self {
        self.memory = Some(memory);
        self
    }

    /// Creates a `VMHostFunctions` instance to be imported by the guest module.
    ///
    /// This method builds the self-referential struct that provides guest code
    /// with access to host capabilities.
    ///
    /// # Panics
    ///
    /// Panics if `with_memory` has not been called first.
    ///
    /// # Arguments
    ///
    /// * `store` - A mutable view of the Wasmer store.
    pub fn host_functions(&'a mut self, store: wasmer::StoreMut<'a>) -> VMHostFunctions<'a> {
        // TODO: review the `clone()` and figure out if the function should be a one-time call only.
        let memory = self.memory.clone().expect("VM Memory not initialized");

        VMHostFunctionsBuilder {
            logic: self,
            store,

            memory_builder: |store| memory.view(store),
        }
        .build()
    }
}

/// Represents the final outcome of a VM execution.
///
/// This struct aggregates all the results and side effects of function calls,
/// such as the return value, logs, events, and state changes.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Outcome {
    /// The result of the execution. `Ok(Some(value))` for a successful return,
    /// `Ok(None)` for no return, and `Err` for a trap or execution error.
    pub returns: VMLogicResult<Option<Vec<u8>>, FunctionCallError>,
    /// All log messages generated during the execution.
    pub logs: Vec<String>,
    /// All events emitted during the execution.
    pub events: Vec<Event>,
    /// The new state root hash if there were commits during the execution.
    pub root_hash: Option<[u8; DIGEST_SIZE]>,
    /// The binary artifact produced if there were commits during the execution.
    //TODO: why the artifact is not an Option?
    pub artifact: Vec<u8>,
    /// A map of proposals created during execution, having proposal ID as a key.
    pub proposals: BTreeMap<[u8; DIGEST_SIZE], Vec<u8>>,
    /// A list of approvals submitted during execution.
    pub approvals: Vec<[u8; DIGEST_SIZE]>,
    //TODO: execution runtime (???).
    //TODO: current storage usage of the app (???).
}

impl VMLogic<'_> {
    /// Consumes the `VMLogic` instance and produces the final `Outcome`.
    ///
    /// This method should be called after the guest function has finished executing
    /// or has trapped. It packages the final state of the `VMLogic` into an
    /// `Outcome` struct.
    ///
    /// # Arguments
    ///
    /// * `err` - An optional `FunctionCallError` that occurred during execution (e.g., a trap).
    ///           If `None`, the outcome is determined by the `returns` field.
    #[must_use]
    pub fn finish(self, err: Option<FunctionCallError>) -> Outcome {
        let returns = match err {
            Some(err) => Err(err),
            None => self
                .returns
                .map(|t| t.map_err(FunctionCallError::ExecutionError))
                .transpose(),
        };

        Outcome {
            returns,
            logs: self.logs,
            events: self.events,
            root_hash: self.root_hash,
            artifact: self.artifact,
            proposals: self.proposals,
            approvals: self.approvals,
        }
    }
}

/// A self-referential struct that holds the `VMLogic`, the Wasmer `StoreMut`,
/// and a `MemoryView` derived from them.
///
/// This structure is necessary to safely provide guest access to host functions,
/// hold the reference to guest memory (`MemoryView`) simultaneously. The `ouroboros`
/// crate ensures that the lifetimes are managed correctly.
#[self_referencing]
pub struct VMHostFunctions<'a> {
    logic: &'a mut VMLogic<'a>,
    store: wasmer::StoreMut<'a>,

    #[covariant]
    #[borrows(store)]
    memory: wasmer::MemoryView<'this>,
}

// Private helper functions for memory access.
impl VMHostFunctions<'_> {
    /// Reads an IMMUTABLE slice of guest memory.
    /// This should be used when the host needs to READ from a buffer
    /// provided by the guest.
    ///
    /// # Arguments
    ///
    /// * `slice` - A `sys::Buffer` descriptor pointing to the buffer location and length in guest memory.
    ///
    /// # Returns
    ///
    /// * Immutable slice of the guest memory contents.
    fn read_guest_memory_slice(&self, slice: &sys::Buffer<'_>) -> &[u8] {
        let ptr = slice.ptr().value().as_usize();
        let len = slice.len() as usize;

        unsafe { &self.borrow_memory().data_unchecked()[ptr..ptr + len] }
    }

    /// Reads a MUTABLE slice of guest memory.
    /// This should only be used when the host needs to WRITE into a buffer
    /// provided by the guest.
    ///
    /// # Arguments
    ///
    /// * `slice` - A `sys::Buffer` descriptor pointing to the buffer location and length in guest memory.
    ///
    /// # Returns
    ///
    /// * Mutable slice of the guest memory contents.
    #[allow(
        clippy::mut_from_ref,
        reason = "We are not modifying the self explicitly, only the underlying slice of the guest memory.\
        Meantime we are required to have an immutable reference to self, hence the exception"
    )]
    fn read_guest_memory_slice_mut(&self, slice: &sys::BufferMut<'_>) -> &mut [u8] {
        let ptr = slice.ptr().value().as_usize();
        let len = slice.len() as usize;

        unsafe { &mut self.borrow_memory().data_unchecked_mut()[ptr..ptr + len] }
    }

    /// Reads an immutable UTF-8 string slice from guest memory.
    ///
    /// # Arguments
    ///
    /// * `slice` - A `sys::Buffer` descriptor pointing to the buffer location and length in guest memory.
    ///
    /// # Returns
    ///
    /// * `VMLogicResult(Ok(utf8_slice))`
    ///
    /// # Errors
    ///
    /// Returns `VMLogicError::HostError(HostError::BadUTF8)` if the memory slice
    /// does not contain valid UTF-8 data.
    fn read_guest_memory_str(&self, slice: &sys::Buffer<'_>) -> VMLogicResult<&str> {
        let buf = self.read_guest_memory_slice(slice);

        std::str::from_utf8(buf).map_err(|_| HostError::BadUTF8.into())
    }

    /// Reads a fixed-size IMMUTABLE array slice from guest memory.
    ///
    /// # Arguments
    ///
    /// * `slice` - A `sys::Buffer` descriptor pointing to the buffer location and length in guest memory.
    ///
    /// # Errors
    ///
    /// Returns `VMLogicError::HostError(HostError::InvalidMemoryAccess)` if the buffer
    /// length in guest memory does not exactly match the requested array size `N`.
    fn read_guest_memory_sized<const N: usize>(
        &self,
        slice: &sys::Buffer<'_>,
    ) -> VMLogicResult<&[u8; N]> {
        let buf = self.read_guest_memory_slice(slice);

        buf.try_into()
            .map_err(|_| HostError::InvalidMemoryAccess.into())
    }

    /// Reads a sized type from guest memory.
    /// Reads a `Sized` type `T` from a specified location in guest memory.
    ///
    /// # Arguments
    ///
    /// * `ptr` - The memory address in the guest's linear memory where the type `T` is located.
    ///
    /// # Errors
    ///
    /// Can return a `Host::InvalidMemoryAccesssError` if the read operation goes out of bounds.
    ///
    /// # Safety
    ///
    /// This function is unsafe because:
    /// 1. It reads raw bytes from guest memory and interprets them as type `T`. The caller
    ///    must ensure that the bytes at `ptr` represent a valid instance of `T`.
    /// 2. It relies on `ptr` being a valid, aligned pointer within the guest memory.
    //TODO: refactor to use `sys::Buffer` instead of `ptr`.
    unsafe fn read_guest_memory_typed<T>(&self, ptr: u64) -> VMLogicResult<T> {
        let mut value = MaybeUninit::<T>::uninit();

        let raw = slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), size_of::<T>());

        self.borrow_memory().read(ptr, raw)?;

        Ok(value.assume_init())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Key, Value};
    use core::ops::Deref;
    use wasmer::{AsStoreMut, Store};

    // The descriptor has the size of 16-bytes with the layout `{ ptr: u64, len: u64 }`
    // See below: [`prepare_guest_buf_descriptor`]
    pub const DESCRIPTOR_SIZE: usize = u64::BITS as usize / 8 * 2;

    // This implementation is more suitable for testing host-side components
    // in comparison to `store::MockedStorage` which is a better for guest-side
    // tests - e.g. testing Calimero application contracts.
    // This version minimally satisfies the `Storage` trait without introducing
    // the global state, guaranteed having a proper test isolation and not having a risk
    // to collide with other tests due to developer error, too (i.e. developer accidentally
    // used the same scope for the global state of `store::MockedStorage` in two different
    // tests).
    pub struct SimpleMockStorage {
        data: HashMap<Vec<u8>, Vec<u8>>,
    }

    impl SimpleMockStorage {
        pub fn new() -> Self {
            Self {
                data: HashMap::new(),
            }
        }
    }

    impl Storage for SimpleMockStorage {
        fn get(&self, key: &Key) -> Option<Value> {
            self.data.get(key).cloned()
        }

        fn set(&mut self, key: Key, value: Value) -> Option<Value> {
            self.data.insert(key, value)
        }

        fn remove(&mut self, key: &Key) -> Option<Value> {
            self.data.remove(key)
        }

        fn has(&self, key: &Key) -> bool {
            self.data.contains_key(key)
        }
    }

    /// A macro to set up the VM environment within a test.
    /// It takes references to storage and limits, which are owned by the test function,
    /// ensuring that all lifetimes are valid.
    macro_rules! setup_vm {
        ($storage:expr, $limits:expr, $input:expr) => {{
            let context =
                VMContext::new(Cow::Owned($input), [0u8; DIGEST_SIZE], [0u8; DIGEST_SIZE]);
            let mut store = Store::default();
            let memory =
                wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
            let mut logic = VMLogic::new($storage, context, $limits, None);
            let _ = logic.with_memory(memory);
            (logic, store)
        }};
    }

    // Export macros to the module level, so it can be reused in other host functions' tests.
    pub(super) use setup_vm;

    /// Helper to write a similar to `sys::Buffer` struct representation to memory.
    /// Simulates a WASM guest preparing a memory descriptor for a host call.
    ///
    /// # Why this is necessary
    /// When a WASM guest needs the host to read/write a slice of its memory, it cannot
    /// pass a slice directly. Instead, it must pass a pointer to a "descriptor" structure
    /// that exists within guest's memory. This descriptor tells the host where the
    /// actual data is (`ptr`) and how long it is (`len`). This function simulates the guest
    /// writing that descriptor into the mock memory.
    ///
    /// # Parameters
    /// - `host`: A reference to the `VMHostFunctions` to get access to the guest memory view.
    /// - `offset`: The address of the descriptor struct itself in the guest memory.
    ///   This is the pointer that the guest would pass to the host function.
    /// - `ptr`: The address of the actual data payload (e.g., a string or byte array) in the
    ///   guest memory. This value is written inside the descriptor structure.
    /// - `len`: The length of the data payload. This value is also written inside the
    ///   descriptor structure.
    ///
    /// # ABI and Memory Layout
    /// Although the guest is `wasm32` and uses `u32` pointers internally, the host-guest
    /// ABI often standardizes on `u64` for all pointers and lengths for consistency and
    /// forward-compatibility with `wasm64`. Therefore, this function writes both `ptr` and `len`
    /// as `u64`, creating a 16-byte descriptor in memory with the layout `{ ptr: u64, len: u64 }`.
    /// All values are little-endian, as required by the WebAssembly specification.
    pub fn prepare_guest_buf_descriptor(
        host: &VMHostFunctions<'_>,
        offset: u64,
        ptr: u64,
        len: u64,
    ) {
        let data: Vec<u8> = [ptr.to_le_bytes(), len.to_le_bytes()].concat();

        host.borrow_memory()
            .write(offset, &data)
            .expect("Failed to write buffer");
    }

    /// A test helper to write a string slice directly into the guest's mock memory.
    ///
    /// This simulates the guest having string data (e.g., a log message, a storage key)
    /// in its linear memory, making it available for the host to read.
    ///
    /// # Parameters
    /// - `host`: A reference to the `VMHostFunctions` to get access to the guest memory view.
    /// - `offset`: The memory address where the string's byte data will be written.
    /// - `s`: The string slice to write into the guest's memory.
    pub fn write_str(host: &VMHostFunctions<'_>, offset: u64, s: &str) {
        host.borrow_memory()
            .write(offset, s.as_bytes())
            .expect("Failed to write string");
    }

    /// A simple sanity check to ensure the default `VMLimits` are configured as expected.
    /// This test helps prevent accidental changes to the default limits.
    #[test]
    fn test_default_limits() {
        let limits = VMLimits::default();
        assert_eq!(limits.max_memory_pages, 1 << 10);
        assert_eq!(limits.max_stack_size, 200 << 10);
        assert_eq!(limits.max_registers, 100);
        assert_eq!(*limits.max_register_size.deref(), 100 << 20);
        assert_eq!(limits.max_registers_capacity, 1 << 30); // 1 GiB
        assert_eq!(limits.max_logs, 100);
        assert_eq!(limits.max_log_size, 16 << 10); // 16 KiB
        assert_eq!(limits.max_events, 100);
        assert_eq!(limits.max_event_kind_size, 100);
        assert_eq!(limits.max_event_data_size, 16 << 10); // 16 KiB
        assert_eq!(limits.max_storage_key_size.get(), 1 << 20); // 1 MiB
        assert_eq!(limits.max_storage_value_size.get(), 10 << 20); // 10 MiB
        assert_eq!(limits.max_blob_handles, 100);
        assert_eq!(limits.max_blob_chunk_size, 10 << 20); // 10 MiB
    }

    /// A smoke test for the successful path of the `finish` method.
    ///
    /// This test simulates a VM execution that successfully finished by
    /// calling `finish(None)` and asserts that the `returns` field in
    /// the final `Outcome` is an `Ok`, ensuring the error is propagated correctly.
    #[test]
    fn test_smoke_finish() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _) = setup_vm!(&mut storage, &limits, vec![1, 2, 3]);
        let outcome = logic.finish(None);

        assert!(outcome.returns.is_ok());
    }

    /// A smoke test for the error-handling path of the `finish` method.
    ///
    /// This test simulates a VM execution that failed by calling `finish(Some(Error))`
    /// and asserts that the `returns` field in the final `Outcome` is an `Err`,
    /// ensuring the error is propagated correctly.
    #[test]
    fn test_smoke_finish_with_error() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _) = setup_vm!(&mut storage, &limits, vec![]);
        let outcome = logic.finish(Some(FunctionCallError::ExecutionError(vec![])));
        assert!(outcome.returns.is_err());
    }

    // ===========================================================================
    // Tests for private functions
    // ===========================================================================

    /// Verifies the success path of the private `read_guest_memory_slice` and `read_guest_memory_str` functions.
    #[test]
    fn test_private_read_guest_memory_slice_and_str_success() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        let expected_str = "hello world";
        let data_ptr = 100u64;
        // Write msg to guest memory.
        write_str(&host, data_ptr, expected_str);
        let buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, expected_str.len() as u64);

        // Use `read_guest_memory_typed` to get a `sys::Buffer` instance,
        // just like public host functions do internally.
        let buffer = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr)
                .unwrap()
        };

        // Guest: ask host to read str from the `buffer` located in guest memory.
        let result_str = host.read_guest_memory_str(&buffer).unwrap();
        assert_eq!(result_str, expected_str);

        // Guest: ask host to read slice from the `buffer` located in guest memory.
        let result_slice = host.read_guest_memory_slice(&buffer);
        assert_eq!(result_slice, expected_str.as_bytes());
    }

    /// Verifies the `read_guest_memory_slice` function can't modify the guest buffer.
    #[test]
    fn test_private_read_guest_memory_slice_unmutable() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        let expected_str = "hello world";
        let data_ptr = 100u64;
        // Write msg to guest memory.
        write_str(&host, data_ptr, expected_str);
        let buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, expected_str.len() as u64);

        // Use `read_guest_memory_typed` to get a `sys::Buffer` instance,
        // just like public host functions do internally.
        let buffer = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr)
                .unwrap()
        };

        // Guest: ask host to read slice from the `buffer` located in guest memory.
        let result_slice = host.read_guest_memory_slice(&buffer);
        assert_eq!(result_slice, expected_str.as_bytes());

        // Now, this code won't be compilable as we get an immutable ref.
        // Host: modify the memory (this could happen accidentally and not intended).
        // ```compile_fail
        // for value in result_slice.iter() {
        // }
        // ```

        // Guest: ask host to read str from the `buffer` located in guest memory.
        let result_str = host.read_guest_memory_str(&buffer).unwrap();
        assert_eq!(result_str, expected_str);
    }

    /// Verifies the error handling of the private `read_guest_memory_str` function for invalid UTF-8.
    #[test]
    fn test_private_read_guest_memory_str_invalid_utf8() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        let invalid_utf8: &[u8] = &[0, 159, 146, 150];
        let data_ptr = 100u64;
        // Write invalid utf8 buffer to the guest memory
        host.borrow_memory().write(data_ptr, invalid_utf8).unwrap();
        let buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, invalid_utf8.len() as u64);

        // Use `read_guest_memory_typed` to get a `sys::Buffer` instance,
        // just like public host functions do internally.
        let buffer = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr)
                .unwrap()
        };

        // Test that `read_guest_memory_str` fails as expected.
        let err = host.read_guest_memory_str(&buffer).unwrap_err();
        assert!(matches!(err, VMLogicError::HostError(HostError::BadUTF8)));
    }

    /// Verifies the success and failure paths of the private `read_guest_memory_sized` function.
    #[test]
    fn test_private_read_guest_memory_sized() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        // Test success case.
        let correct_data = [42u8; DIGEST_SIZE];
        let data_ptr_ok = 100u64;
        // Write correct data to guest memory.
        host.borrow_memory()
            .write(data_ptr_ok, &correct_data)
            .unwrap();
        let buf_ptr_ok = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, buf_ptr_ok, data_ptr_ok, correct_data.len() as u64);

        // Use `read_guest_memory_typed` to get a `sys::Buffer` instance,
        // just like public host functions do internally.
        let buffer_ok = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr_ok)
                .unwrap()
        };
        let result_sized_ok = host
            .read_guest_memory_sized::<DIGEST_SIZE>(&buffer_ok)
            .unwrap();
        assert_eq!(result_sized_ok, &correct_data);

        // Test failure case (incorrect length).
        let incorrect_data = [1u8; 31];
        let data_ptr_err = 300u64;
        // Write incorrect data to guest memory.
        host.borrow_memory()
            .write(data_ptr_err, &incorrect_data)
            .unwrap();
        let buf_ptr_err = 32u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            buf_ptr_err,
            data_ptr_err,
            incorrect_data.len() as u64,
        );

        // Use `read_guest_memory_typed` to get a `sys::Buffer` instance,
        // just like public host functions do internally.
        let buffer_err = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr_err)
                .unwrap()
        };
        // Guest: ask host to read the guest memory sized.
        let err = host
            .read_guest_memory_sized::<DIGEST_SIZE>(&buffer_err)
            .unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));
    }
}
