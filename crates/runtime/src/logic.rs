#![allow(single_use_lifetimes, unused_lifetimes, reason = "False positive")]
#![allow(clippy::mem_forget, reason = "Safe for now")]

use core::mem::MaybeUninit;
use core::num::NonZeroU64;
use core::{fmt, slice};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};
use std::vec;

use borsh::from_slice as from_borsh_slice;
use calimero_node_primitives::client::NodeClient;
use calimero_sys as sys;
use ouroboros::self_referencing;
use rand::RngCore;
use serde::Serialize;

use crate::constraint::{Constrained, MaxU64};
use crate::errors::{FunctionCallError, HostError, Location, PanicContext};
use crate::store::Storage;
use crate::Constraint;

mod errors;
mod imports;
mod registers;
mod host_functions;

pub use errors::VMLogicError;
pub use host_functions::*;
use registers::Registers;

/// A specialized `Result` type for VMLogic operations.
pub type VMLogicResult<T, E = VMLogicError> = Result<T, E>;

/// The standard size of the digest used in bytes.
/// The digest is used everywhere: for context, public key, proposals, etc.
const DIGEST_SIZE: usize = 32;

// The constant for one kibibyte for a better readability and less error-prone approach on usage.
const ONE_KIB: u32 = 1024;
// The constant for one mibibyte for a better readability and less error-prone approach on usage.
const ONE_MIB: u32 = ONE_KIB * 1024;
// The constant for one gibibyte for a better readability and less error-prone approach on usage.
const ONE_GIB: u32 = ONE_MIB * 1024;

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

/// Represents a structured event emitted during the execution.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Event {
    /// A string identifying the type or category of the event.
    pub kind: String,
    /// The binary data payload associated with the event.
    pub data: Vec<u8>,
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
    unsafe fn read_typed<T>(&self, ptr: u64) -> VMLogicResult<T> {
        let mut value = MaybeUninit::<T>::uninit();

        let raw = slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), size_of::<T>());

        self.borrow_memory().read(ptr, raw)?;

        Ok(value.assume_init())
    }
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
        let location = unsafe { self.read_typed::<sys::Location<'_>>(src_location_ptr)? };

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
        let panic_message_buf = unsafe { self.read_typed::<sys::Buffer<'_>>(src_panic_msg_ptr)? };
        let location = unsafe { self.read_typed::<sys::Location<'_>>(src_location_ptr)? };

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
        let dest_data = unsafe { self.read_typed::<sys::BufferMut<'_>>(dest_data_ptr)? };

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
        let result = unsafe { self.read_typed::<sys::ValueReturn<'_>>(src_value_ptr)? };

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
        let src_log_buf = unsafe { self.read_typed::<sys::Buffer<'_>>(src_log_ptr)? };

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
        let event = unsafe { self.read_typed::<sys::Event<'_>>(src_event_ptr)? };

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
        let root_hash = unsafe { self.read_typed::<sys::Buffer<'_>>(src_root_hash_ptr)? };
        let artifact = unsafe { self.read_typed::<sys::Buffer<'_>>(src_artifact_ptr)? };

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


    /// Fetches data from a URL.
    ///
    /// Performs an HTTP request. This is a synchronous, blocking operation.
    /// The response body is placed into the specified host register.
    ///
    /// # Arguments
    ///
    /// * `src_url_ptr` - Pointer to the URL string source-buffer in guest memory.
    /// * `src_method_ptr` - Pointer to the HTTP method string source-buffer (e.g., "GET", "POST")
    /// in guest memory.
    /// * `src_headers_ptr` - Pointer to a borsh-serialized `Vec<(String, String)>` source-buffer of
    /// headers in guest memory.
    /// * `src_body_ptr` - Pointer to the request body source-buffer in guest memory.
    /// * `dest_register_id` - The ID of the destination register in host memory where to store
    /// the response body.
    ///
    /// # Returns
    ///
    /// * Returns `0` on success (HTTP 2xx)
    /// * Returns `1` on failure.
    /// The response body or error message is placed in the host register.
    ///
    /// # Errors
    ///
    /// * `HostError::DeserializationError` if the headers cannot be deserialized.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn fetch(
        &mut self,
        src_url_ptr: u64,
        src_method_ptr: u64,
        src_headers_ptr: u64,
        src_body_ptr: u64,
        dest_register_id: u64,
    ) -> VMLogicResult<u32> {
        let url = unsafe { self.read_typed::<sys::Buffer<'_>>(src_url_ptr)? };
        let method = unsafe { self.read_typed::<sys::Buffer<'_>>(src_method_ptr)? };
        let headers = unsafe { self.read_typed::<sys::Buffer<'_>>(src_headers_ptr)? };
        let body = unsafe { self.read_typed::<sys::Buffer<'_>>(src_body_ptr)? };

        let url = self.read_guest_memory_str(&url)?;
        let method = self.read_guest_memory_str(&method)?;

        let headers = self.read_guest_memory_slice(&headers);
        let body = self.read_guest_memory_slice(&body);

        // TODO: clarify why the `fetch` function cannot be directly called by applications.
        // Note: The `fetch` function cannot be directly called by applications.
        // Therefore, the headers are generated exclusively by our code, ensuring
        // that it is safe to deserialize them.
        let headers: Vec<(String, String)> =
            from_borsh_slice(headers).map_err(|_| HostError::DeserializationError)?;

        let mut request = ureq::request(&method, &url);

        for (key, value) in &headers {
            request = request.set(key, value);
        }

        let response = if body.is_empty() {
            request.call()
        } else {
            request.send_bytes(body)
        };

        let (status, data) = match response {
            Ok(response) => {
                let mut buffer = vec![];
                match response.into_reader().read_to_end(&mut buffer) {
                    Ok(_) => (0, buffer),
                    Err(_) => (1, "Failed to read the response body.".into()),
                }
            }
            Err(e) => (1, e.to_string().into_bytes()),
        };

        self.with_logic_mut(|logic| logic.registers.set(logic.limits, dest_register_id, data))?;
        Ok(status)
    }

    /// Fills a guest memory buffer with random bytes.
    ///
    /// # Arguments
    ///
    /// * `dest_ptr` - A destination pointer to a `sys::BufferMut` in guest memory to be filled.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for a descriptor buffer.
    pub fn random_bytes(&mut self, dest_ptr: u64) -> VMLogicResult<()> {
        let dest_buf = unsafe { self.read_typed::<sys::BufferMut<'_>>(dest_ptr)? };

        rand::thread_rng().fill_bytes(self.read_guest_memory_slice_mut(&dest_buf));

        Ok(())
    }

    /// Gets the current Unix timestamp in nanoseconds.
    ///
    /// This function obtains the current time as a nanosecond timestamp, as
    /// [`SystemTime`] is not available inside the guest runtime. Therefore the
    /// guest needs to request this from the host.
    ///
    /// The result is written into a guest buffer as a `u64`.
    ///
    /// # Arguments
    ///
    /// * `dest_ptr` - A pointer to an 8-byte destination buffer `sys::BufferMut`
    /// in guest memory where the `u64` timestamp will be written.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the provided buffer is not exactly 8 bytes long
    /// or if memory access fails for a descriptor buffer.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Impossible to overflow in normal circumstances"
    )]
    #[expect(
        clippy::expect_used,
        clippy::unwrap_in_result,
        reason = "Effectively infallible here"
    )]
    pub fn time_now(&mut self, dest_ptr: u64) -> VMLogicResult<()> {
        let guest_time_ptr = unsafe { self.read_typed::<sys::BufferMut<'_>>(dest_ptr)? };

        if guest_time_ptr.len() != 8 {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64;

        // Record the time into the guest memory buffer
        let guest_time_out_buf: &mut [u8] = self.read_guest_memory_slice_mut(&guest_time_ptr);
        guest_time_out_buf.copy_from_slice(&now.to_le_bytes());

        Ok(())
    }

    /// Creates a new governance proposal.
    ///
    /// Call the contract's `send_proposal()` function through the bridge.
    ///
    /// The proposal actions are obtained as raw data and pushed onto a list of
    /// proposals to be sent to the host.
    ///
    /// Note that multiple actions are received, and the entire batch is pushed
    /// onto the proposal list to represent one proposal.
    ///
    /// A unique ID for the proposal is generated by the host and written back into
    /// guest memory. The proposal itself is stored in the `VMLogic` to be included
    /// in the `Outcome`.
    ///
    /// # Arguments
    ///
    /// * `src_actions_ptr` - pointer to a source-buffer `sys::Buffer` in guest memory,
    /// containing the proposal's actions.
    /// * `dest_id_ptr` - A pointer to a 32-byte destination buffer `sys::BufferMut`
    /// in guest memory where the generated proposal ID will be written.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn send_proposal(&mut self, src_actions_ptr: u64, dest_id_ptr: u64) -> VMLogicResult<()> {
        let actions = unsafe { self.read_typed::<sys::Buffer<'_>>(src_actions_ptr)? };
        let dest_id = unsafe { self.read_typed::<sys::BufferMut<'_>>(dest_id_ptr)? };

        let mut proposal_id = [0u8; DIGEST_SIZE];
        rand::thread_rng().fill_bytes(&mut proposal_id);

        // Record newly created ID to guest memory
        let dest_id: &mut [u8] = self.read_guest_memory_slice_mut(&dest_id);
        dest_id.copy_from_slice(&proposal_id);

        let actions = self.read_guest_memory_slice(&actions).to_vec();

        let _ignored = self.with_logic_mut(|logic| logic.proposals.insert(proposal_id, actions));
        Ok(())
    }

    /// Approves a governance proposal.
    ///
    /// Adds the given proposal ID to the list of approvals in the `VMLogic`.
    ///
    /// # Arguments
    ///
    /// * `src_approval_ptr` - Pointer to a 32-byte source-buffer `sys::Buffer`
    /// in guest memory containing the ID of the proposal to approve.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn approve_proposal(&mut self, src_approval_ptr: u64) -> VMLogicResult<()> {
        let approval = unsafe { self.read_typed::<sys::Buffer<'_>>(src_approval_ptr)? };
        let approval = *self.read_guest_memory_sized::<DIGEST_SIZE>(&approval)?;

        let _ignored = self.with_logic_mut(|logic| logic.approvals.push(approval));
        Ok(())
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
    const DESCRIPTOR_SIZE: usize = u64::BITS as usize / 8 * 2;

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
            let memory = wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
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
    pub fn prepare_guest_buf_descriptor(host: &VMHostFunctions<'_>, offset: u64, ptr: u64, len: u64) {
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
        let memory = wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
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

    #[test]
    fn test_random_bytes() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let buf_ptr = 10u64;
        let data_ptr = 200u64;
        let data_len = 32u64;

        // Explicitly fill the memory with some pattern before the host call.
        // This makes the test deterministic (for CI) and ensures it fails
        // correctly if the function under this test does not write to the buffer.
        let initial_pattern = vec![0xAB; data_len as usize];
        host.borrow_memory()
            .write(data_ptr, &initial_pattern)
            .unwrap();

        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, data_len);

        // Guest: ask host to fill the buffer with random bytes
        host.random_bytes(buf_ptr).unwrap();

        // Host: perform a priveleged read of the contents of guest's memory to extract the buffer
        // back.
        let mut random_data = vec![0u8; data_len as usize];
        host.borrow_memory()
            .read(data_ptr, &mut random_data)
            .unwrap();

        // Assert that the memory content has changed from our initial pattern.
        assert_ne!(
            random_data, initial_pattern,
            "The data buffer should have been overwritten with random bytes, but it was not."
        );
    }

    /// Tests the `time_now()` host function.
    #[test]
    fn test_time_now() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let buf_ptr = 16u64;
        let time_data_ptr = 200u64;
        // The `time_now()` function expects an 8-byte buffer to write the u64 timestamp.
        let time_data_len = u64::BITS as u64 / 8;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, buf_ptr, time_data_ptr, time_data_len);

        // Record the host's system time before the host-function call.
        let time_before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Guest: ask the host to provide the current timestamp and write it to the buffer.
        host.time_now(buf_ptr).unwrap();

        // Record the host's system time after the host-function call.
        let time_after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Host: read the timestamp back from guest memory.
        let mut time_buffer = [0u8; 8];
        host.borrow_memory()
            .read(time_data_ptr, &mut time_buffer)
            .unwrap();
        let timestamp_from_host = u64::from_le_bytes(time_buffer);

        // Verify the timestamp is current and valid (within the before-after range).
        assert!(timestamp_from_host >= time_before);
        assert!(timestamp_from_host <= time_after);
    }

    /// Tests the `send_proposal()` and `approve_proposal()` host functions.
    #[test]
    fn test_proposals_send_approve() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Test sending a proposal.
        let actions = vec![1, 2, 3, 4, 5, 6];
        let actions_ptr = 100u64;
        // Write actions to guest memory.
        host.borrow_memory().write(actions_ptr, &actions).unwrap();
        let actions_buf_ptr = 16u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(&host, actions_buf_ptr, actions_ptr, actions.len() as u64);

        let id_out_ptr = 300u64;
        let id_buf_ptr = 32u64;
        // Guest: prepare the descriptor for the destination buffer so host can write there.
        prepare_guest_buf_descriptor(&host, id_buf_ptr, id_out_ptr, DIGEST_SIZE as u64);
        // Guest: send proposal to host with actions `actions_buf_ptr` and get back the proposal ID
        // in `id_buf_ptr`.
        host.send_proposal(actions_buf_ptr, id_buf_ptr).unwrap();

        // Verify the proposal with the given actions were successfully added.
        assert_eq!(host.borrow_logic().proposals.len(), 1);
        assert_eq!(
            host.borrow_logic().proposals.values().next().unwrap(),
            &actions
        );
        // Verify there are no approvals yet.
        assert_eq!(host.borrow_logic().approvals.len(), 0);

        // Test approving a proposal.
        // Approval ID is the Answer to the Ultimate Question of Life, the Universe, and Everything.
        let approval_id = [42u8; DIGEST_SIZE];
        let approval_ptr = 500u64;
        // Write approval to guest memory.
        host.borrow_memory()
            .write(approval_ptr, &approval_id)
            .unwrap();

        let approval_buf_ptr = 48u64;
        // Guest: prepare the descriptor for the destination buffer so host can access it.
        prepare_guest_buf_descriptor(
            &host,
            approval_buf_ptr,
            approval_ptr,
            approval_id.len() as u64,
        );

        // Guest: send a proposal approval to host.
        host.approve_proposal(approval_buf_ptr).unwrap();

        // Verify the host successfully stored the approval and its ID matches the one we sent.
        assert_eq!(host.borrow_logic().approvals.len(), 1);
        assert_eq!(host.borrow_logic().approvals[0], approval_id);
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

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr).unwrap() };

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

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr).unwrap() };

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

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr).unwrap() };

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

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer_ok = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr_ok).unwrap() };
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

        // Use `read_typed` to get a `sys::Buffer` instance, just like public host functions
        // do internally.
        let buffer_err = unsafe { host.read_typed::<sys::Buffer<'_>>(buf_ptr_err).unwrap() };
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
