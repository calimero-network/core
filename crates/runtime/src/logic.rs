#![allow(single_use_lifetimes, unused_lifetimes, reason = "False positive")]
#![allow(clippy::mem_forget, reason = "Safe for now")]

use core::mem::MaybeUninit;
use core::num::NonZeroU64;
use core::{fmt, slice};
use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::vec;

use tracing::{debug, trace};

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::common::DIGEST_SIZE;
use calimero_sys as sys;
use ouroboros::self_referencing;
use serde::{Deserialize, Serialize};

use crate::constants::{ONE_GIB, ONE_KIB, ONE_MIB};
use crate::constraint::{Constrained, MaxU64};
use crate::errors::{FunctionCallError, HostError, Location, PanicContext};
pub use crate::logic::traits::ContextHost;
use crate::store::Storage;
use crate::Constraint;

mod errors;
mod host_functions;
mod imports;
mod registers;
mod traits;

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

// Default limit constants for VMLimits.
// These constants define the default values for VM resource limits,
// making them easier to configure and document.

/// Default maximum stack size in KiB (200 KiB).
const DEFAULT_MAX_STACK_SIZE_KIB: usize = 200;
/// Default maximum number of registers.
const DEFAULT_MAX_REGISTERS: u64 = 100;
/// Default maximum register size in MiB (100 MiB).
const DEFAULT_MAX_REGISTER_SIZE_MIB: u64 = 100;
/// Default maximum number of log entries.
const DEFAULT_MAX_LOGS: u64 = 100;
/// Default maximum log message size in KiB (16 KiB).
const DEFAULT_MAX_LOG_SIZE_KIB: u64 = 16;
/// Default maximum number of events.
const DEFAULT_MAX_EVENTS: u64 = 100;
/// Default maximum event kind size in bytes.
const DEFAULT_MAX_EVENT_KIND_SIZE: u64 = 100;
/// Default maximum event data size in KiB (16 KiB).
const DEFAULT_MAX_EVENT_DATA_SIZE_KIB: u64 = 16;
/// Default maximum number of cross-context calls.
const DEFAULT_MAX_XCALLS: u64 = 100;
/// Default maximum cross-context call function name size in bytes.
const DEFAULT_MAX_XCALL_FUNCTION_SIZE: u64 = 100;
/// Default maximum cross-context call parameters size in KiB (16 KiB).
const DEFAULT_MAX_XCALL_PARAMS_SIZE_KIB: u64 = 16;
/// Default maximum storage key size in MiB (1 MiB).
const DEFAULT_MAX_STORAGE_KEY_SIZE_MIB: u64 = 1;
/// Default maximum storage value size in MiB (10 MiB).
const DEFAULT_MAX_STORAGE_VALUE_SIZE_MIB: u64 = 10;
/// Default maximum number of blob handles.
const DEFAULT_MAX_BLOB_HANDLES: u64 = 100;
/// Default maximum blob chunk size in MiB (10 MiB).
const DEFAULT_MAX_BLOB_CHUNK_SIZE_MIB: u64 = 10;
/// Default maximum method name length in bytes.
const DEFAULT_MAX_METHOD_NAME_LENGTH: u64 = 256;
/// Default maximum WASM module size in MiB (10 MiB).
const DEFAULT_MAX_MODULE_SIZE_MIB: u64 = 10;
/// Default maximum operations for WASM execution (300 million).
///
/// This limits how many WASM operations can be executed before termination.
/// A value of 0 disables the execution limit.
///
/// **Practical guidance:** On typical hardware, 300M operations corresponds to
/// roughly 10-60 seconds of wall-clock time, depending on the operation mix.
/// Simple arithmetic loops execute faster than memory-intensive operations.
/// Start with the default and adjust based on your workload's profiling data.
const DEFAULT_MAX_OPERATIONS: u64 = 300_000_000;

/// Defines the resource limits for a VM instance.
///
/// This struct is used to configure constraints on various VM operations to prevent
/// excessive resource consumption.
///
/// Note: New fields should be added at the end for better forward compatibility
/// if serialization is ever added.
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
    /// The maximum number of cross-context calls that can be made.
    pub max_xcalls: u64,
    /// The maximum size of a cross-context call function name in bytes.
    pub max_xcall_function_size: u64,
    /// The maximum size of cross-context call parameters in bytes.
    pub max_xcall_params_size: u64,
    /// The maximum size of a storage key in bytes.
    pub max_storage_key_size: NonZeroU64,
    /// The maximum size of a storage value in bytes.
    pub max_storage_value_size: NonZeroU64,
    /// The maximum number of blob handles that can exist.
    pub max_blob_handles: u64,
    /// The maximum size of a single chunk when writing to or reading from a blob.
    pub max_blob_chunk_size: u64,
    /// The maximum length of a method name in bytes.
    pub max_method_name_length: u64,
    /// The maximum size of a WASM module in bytes before compilation.
    /// This limit prevents memory exhaustion attacks from large malicious modules.
    /// Setting this to 0 will reject all non-empty modules.
    ///
    /// **Configuration guidance**: Typical WASM modules range from 100 KiB to a few MiB.
    /// The default of 10 MiB accommodates most applications while preventing memory
    /// exhaustion. Consider reducing for memory-constrained environments.
    pub max_module_size: u64,
    /// The maximum number of WASM operations allowed per execution.
    ///
    /// Each WASM instruction counts as one operation using a **uniform cost function**.
    /// This means all operations (arithmetic, memory access, calls, branches) have equal
    /// cost. While this doesn't perfectly model wall-clock time, it provides predictable
    /// and deterministic execution limits. For production use, consider adding a safety
    /// margin to account for operation mix variability.
    ///
    /// When the limit is reached, execution is terminated with an `ExecutionTimeout` error.
    /// This prevents infinite loops and long-running computations from blocking
    /// the executor indefinitely.
    ///
    /// **Note:** The operation budget is shared between all WASM function calls in a single
    /// execution, including registration hooks like `__calimero_register_merge`. If hooks
    /// consume operations, the remaining budget for the user method is reduced accordingly.
    ///
    /// **Practical guidance:** The default of 300M operations typically corresponds to
    /// 10-60 seconds of execution time depending on workload. Monitor actual execution
    /// stats (logged at debug level) to tune this value for your specific use case.
    ///
    /// A value of 0 disables the execution limit. Use caution when disabling limits for
    /// untrusted code, as infinite loops will block execution indefinitely.
    pub max_operations: u64,
}

impl Default for VMLimits {
    fn default() -> Self {
        #[inline(always)]
        fn is_valid<T, E: fmt::Debug>(t: Result<T, E>) -> T {
            t.expect("is valid")
        }

        Self {
            max_memory_pages: ONE_KIB,
            max_stack_size: DEFAULT_MAX_STACK_SIZE_KIB * ONE_KIB as usize,
            max_registers: DEFAULT_MAX_REGISTERS,
            max_register_size: is_valid(
                (DEFAULT_MAX_REGISTER_SIZE_MIB * u64::from(ONE_MIB)).validate(),
            ),
            max_registers_capacity: u64::from(ONE_GIB),
            max_logs: DEFAULT_MAX_LOGS,
            max_log_size: DEFAULT_MAX_LOG_SIZE_KIB * u64::from(ONE_KIB),
            max_events: DEFAULT_MAX_EVENTS,
            max_event_kind_size: DEFAULT_MAX_EVENT_KIND_SIZE,
            max_event_data_size: DEFAULT_MAX_EVENT_DATA_SIZE_KIB * u64::from(ONE_KIB),
            max_xcalls: DEFAULT_MAX_XCALLS,
            max_xcall_function_size: DEFAULT_MAX_XCALL_FUNCTION_SIZE,
            max_xcall_params_size: DEFAULT_MAX_XCALL_PARAMS_SIZE_KIB * u64::from(ONE_KIB),
            max_storage_key_size: is_valid(
                (DEFAULT_MAX_STORAGE_KEY_SIZE_MIB * u64::from(ONE_MIB)).try_into(),
            ),
            max_storage_value_size: is_valid(
                (DEFAULT_MAX_STORAGE_VALUE_SIZE_MIB * u64::from(ONE_MIB)).try_into(),
            ),
            max_blob_handles: DEFAULT_MAX_BLOB_HANDLES,
            max_blob_chunk_size: DEFAULT_MAX_BLOB_CHUNK_SIZE_MIB * u64::from(ONE_MIB),
            max_method_name_length: DEFAULT_MAX_METHOD_NAME_LENGTH,
            max_module_size: DEFAULT_MAX_MODULE_SIZE_MIB * u64::from(ONE_MIB),
            max_operations: DEFAULT_MAX_OPERATIONS,
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
    /// A list of cross-context calls to be executed.
    xcalls: Vec<XCall>,
    /// The root hash of the state after a successful commit.
    root_hash: Option<[u8; DIGEST_SIZE]>,
    /// A binary artifact produced by the execution.
    artifact: Vec<u8>,
    /// Tracks whether the guest has explicitly called `env.commit`.
    commit_called: bool,
    /// A map of proposals created during execution, having proposal ID as a key.
    proposals: BTreeMap<[u8; DIGEST_SIZE], Vec<u8>>,
    /// A list of approvals submitted during execution.
    approvals: Vec<[u8; DIGEST_SIZE]>,

    /// A list of context configuration mutations requested by the guest.
    context_mutations: Vec<ContextMutation>,
    /// Interface to the host system for querying context information (e.g. membership).
    context_host: Option<Box<dyn ContextHost>>,

    /// An optional client for interacting with the node's blob storage and aliases.
    node_client: Option<NodeClient>,

    // Blob functionality
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
        context_host: Option<Box<dyn ContextHost>>,
    ) -> Self {
        debug!(
            target: "runtime::logic",
            context = ?context,
            limits = ?limits,
            has_node_client = node_client.is_some(),
            "VMLogic::new"
        );
        VMLogic {
            storage,
            memory: None,
            context,
            limits,
            registers: Registers::default(),
            returns: None,
            logs: vec![],
            events: vec![],
            xcalls: vec![],
            root_hash: None,
            artifact: vec![],
            commit_called: false,
            proposals: BTreeMap::new(),
            approvals: vec![],
            context_mutations: vec![],
            context_host,

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

        debug!(target: "runtime::logic", "VMLogic::host_functions: building host function bindings");

        VMHostFunctionsBuilder {
            logic: self,
            store,

            memory_builder: |store| memory.view(store),
        }
        .build()
    }
}

/// Represents a request from the guest to modify context configuration.
///
/// These mutations are collected during execution and applied by the host
/// after the WASM execution completes successfully.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextMutation {
    /// Request to add a new member to the context.
    AddMember { public_key: [u8; DIGEST_SIZE] },
    /// Request to remove an existing member from the context.
    RemoveMember { public_key: [u8; DIGEST_SIZE] },
    /// Request to create a new context.
    CreateContext {
        protocol: String,
        application_id: [u8; DIGEST_SIZE],
        init_args: Vec<u8>,
        alias: Option<String>,
    },
    /// Request to delete a context (locally).
    DeleteContext { context_id: [u8; DIGEST_SIZE] },
    // TODO: add Grant/Revoke capabilities for ACL here in the future.
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
    /// All cross-context calls made during the execution.
    pub xcalls: Vec<XCall>,
    /// The new state root hash if there were commits during the execution.
    pub root_hash: Option<[u8; DIGEST_SIZE]>,
    /// The binary artifact produced if there were commits during the execution.
    //TODO: why the artifact is not an Option?
    pub artifact: Vec<u8>,
    /// A map of proposals created during execution, having proposal ID as a key.
    pub proposals: BTreeMap<[u8; DIGEST_SIZE], Vec<u8>>,
    /// A list of approvals submitted during execution.
    pub approvals: Vec<[u8; DIGEST_SIZE]>,
    /// A list of context mutations submitted during execution.
    pub context_mutations: Vec<ContextMutation>,
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
    pub fn finish(mut self, err: Option<FunctionCallError>) -> Outcome {
        let log_count = self.logs.len();
        let event_count = self.events.len();
        let xcall_count = self.xcalls.len();
        let proposal_count = self.proposals.len();
        let approval_count = self.approvals.len();
        let has_root_hash = self.root_hash.is_some();
        let has_artifact = !self.artifact.is_empty();

        debug!(target: "runtime::internal", "Printing internal WASM LOGS");
        for log in &self.logs {
            debug!(target: "runtime::internal", log);
        }

        let returns = match err {
            Some(err) => Err(err),
            None => self
                .returns
                .map(|t| t.map_err(FunctionCallError::ExecutionError))
                .transpose(),
        };

        debug!(
            target: "runtime::logic",
            has_error = returns.is_err(),
            log_count,
            event_count,
            xcall_count,
            proposal_count,
            approval_count,
            has_root_hash,
            has_artifact,
            "VMLogic::finish"
        );

        // Explicitly clean up WASM memory before returning.
        // This ensures proper cleanup of the memory reference, decrementing
        // Wasmer's internal reference count. The memory field is moved out
        // and dropped here to guarantee cleanup even if the caller doesn't
        // use the Outcome immediately.
        //
        // Note: This cleanup is critical for preventing dangling WASM memory.
        // The runtime's catch_unwind wrapper ensures finish() is always called,
        // even when execution fails mid-way or panics occur.
        if let Some(memory) = self.memory {
            drop(memory);
            trace!(target: "runtime::logic", "VMLogic::finish: cleaned up WASM memory");
        }

        // Clean up any remaining blob handles that weren't properly closed.
        // This prevents resource leaks when guest code fails to close handles.
        if !self.blob_handles.is_empty() {
            let handle_count = self.blob_handles.len();
            let blob_handles = std::mem::take(&mut self.blob_handles);
            drop(blob_handles);
            trace!(
                target: "runtime::logic",
                handle_count,
                "VMLogic::finish: cleaned up remaining blob handles"
            );
        }

        Outcome {
            returns,
            logs: self.logs,
            events: self.events,
            xcalls: self.xcalls,
            root_hash: self.root_hash,
            artifact: self.artifact,
            proposals: self.proposals,
            approvals: self.approvals,
            context_mutations: self.context_mutations,
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
    ///
    /// # Errors
    ///
    /// Returns `VMLogicError::HostError(HostError::InvalidMemoryAccess)` if the requested
    /// memory region (ptr + len) exceeds the bounds of guest memory.
    fn read_guest_memory_slice(&self, slice: &sys::Buffer<'_>) -> VMLogicResult<&[u8]> {
        let ptr = slice.ptr().value().as_usize();
        let len = slice.len() as usize;

        trace!(
            target: "runtime::memory",
            ptr,
            len,
            "read_guest_memory_slice"
        );

        let memory = self.borrow_memory();
        let memory_size = memory.data_size() as usize;

        // Check for potential overflow and bounds
        let end = ptr.checked_add(len).ok_or(HostError::InvalidMemoryAccess)?;
        if end > memory_size {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        // SAFETY: We have verified that ptr..ptr+len is within the memory bounds
        Ok(unsafe { &memory.data_unchecked()[ptr..end] })
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
    ///
    /// # Errors
    ///
    /// Returns `VMLogicError::HostError(HostError::InvalidMemoryAccess)` if the requested
    /// memory region (ptr + len) exceeds the bounds of guest memory.
    #[expect(
        clippy::mut_from_ref,
        reason = "We are not modifying the self explicitly, only the underlying slice of the guest memory.\
        Meantime we are required to have an immutable reference to self, hence the exception"
    )]
    fn read_guest_memory_slice_mut(&self, slice: &sys::BufferMut<'_>) -> VMLogicResult<&mut [u8]> {
        let ptr = slice.ptr().value().as_usize();
        let len = slice.len() as usize;

        trace!(
            target: "runtime::memory",
            ptr,
            len,
            "read_guest_memory_slice_mut"
        );

        let memory = self.borrow_memory();
        let memory_size = memory.data_size() as usize;

        // Check for potential overflow and bounds
        let end = ptr.checked_add(len).ok_or(HostError::InvalidMemoryAccess)?;
        if end > memory_size {
            return Err(HostError::InvalidMemoryAccess.into());
        }

        // SAFETY: We have verified that ptr..ptr+len is within the memory bounds
        Ok(unsafe { &mut memory.data_unchecked_mut()[ptr..end] })
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
    /// Returns `VMLogicError::HostError(HostError::InvalidMemoryAccess)` if the requested
    /// memory region exceeds the bounds of guest memory.
    fn read_guest_memory_str(&self, slice: &sys::Buffer<'_>) -> VMLogicResult<&str> {
        let buf = self.read_guest_memory_slice(slice)?;

        trace!(target: "runtime::memory", len = buf.len(), "read_guest_memory_str");

        core::str::from_utf8(buf).map_err(|_| HostError::BadUTF8.into())
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
    /// length in guest memory does not exactly match the requested array size `N`,
    /// or if the requested memory region exceeds the bounds of guest memory.
    fn read_guest_memory_sized<const N: usize>(
        &self,
        slice: &sys::Buffer<'_>,
    ) -> VMLogicResult<&[u8; N]> {
        let buf = self.read_guest_memory_slice(slice)?;

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
            let mut logic = VMLogic::new($storage, context, $limits, None, None);
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
        assert_eq!(limits.max_module_size, 10 << 20); // 10 MiB
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
        assert_eq!(limits.max_xcalls, 100);
        assert_eq!(limits.max_xcall_function_size, 100);
        assert_eq!(limits.max_xcall_params_size, 16 << 10); // 16 KiB
        assert_eq!(limits.max_storage_key_size.get(), 1 << 20); // 1 MiB
        assert_eq!(limits.max_storage_value_size.get(), 10 << 20); // 10 MiB
        assert_eq!(limits.max_blob_handles, 100);
        assert_eq!(limits.max_blob_chunk_size, 10 << 20); // 10 MiB
        assert_eq!(limits.max_method_name_length, 256);
        assert_eq!(limits.max_operations, 300_000_000); // 300 million operations
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

    /// Tests that VMLogic properly cleans up WASM memory when finish() is called.
    ///
    /// This test verifies that the memory cleanup hook in finish() works correctly,
    /// ensuring that WASM memory is properly released when execution completes.
    /// The cleanup is critical for preventing dangling memory references.
    #[test]
    fn test_vmlogic_finish_cleans_up_memory() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();

        let (logic, _store) = setup_vm!(&mut storage, &limits, vec![]);

        // Verify memory is attached before finish()
        assert!(logic.memory.is_some(), "Memory should be attached");

        // Call finish() which should clean up memory
        let outcome = logic.finish(None);
        assert!(outcome.returns.is_ok());

        // Logic is consumed by finish(), memory was cleaned up inside finish()
        // before the Outcome was returned. This ensures no dangling references.
    }

    /// Tests that VMLogic finish() handles the case where memory was never attached.
    ///
    /// This tests a scenario where VMLogic is created but memory attachment fails,
    /// ensuring finish() handles None memory gracefully without panicking.
    #[test]
    fn test_vmlogic_finish_without_memory() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let context = VMContext::new(Cow::Owned(vec![]), [0u8; DIGEST_SIZE], [0u8; DIGEST_SIZE]);

        // Create VMLogic without attaching memory
        let logic = VMLogic::new(&mut storage, context, &limits, None, None);

        // Verify no memory is attached
        assert!(logic.memory.is_none(), "Memory should not be attached");

        // Call finish() - this should handle None memory gracefully
        let outcome = logic.finish(None);
        assert!(outcome.returns.is_ok());
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
        let result_slice = host.read_guest_memory_slice(&buffer).unwrap();
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
        let result_slice = host.read_guest_memory_slice(&buffer).unwrap();
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

    /// Tests that `read_guest_memory_slice` returns an error when the requested memory region
    /// exceeds the bounds of guest memory.
    #[test]
    fn test_read_guest_memory_slice_out_of_bounds() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        // Get the memory size - one page is 64KB (65536 bytes)
        let memory_size = host.borrow_memory().data_size() as u64;

        // Test case 1: ptr + len exceeds memory size
        let data_ptr = memory_size - 10; // 10 bytes before end of memory
        let buf_ptr = 16u64;
        // Request 100 bytes which will exceed memory bounds
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, 100);

        let buffer = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr)
                .unwrap()
        };

        let err = host.read_guest_memory_slice(&buffer).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));

        // Test case 2: ptr itself is beyond memory size
        let data_ptr_beyond = memory_size + 100;
        let buf_ptr_beyond = 32u64;
        prepare_guest_buf_descriptor(&host, buf_ptr_beyond, data_ptr_beyond, 10);

        let buffer_beyond = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr_beyond)
                .unwrap()
        };

        let err_beyond = host.read_guest_memory_slice(&buffer_beyond).unwrap_err();
        assert!(matches!(
            err_beyond,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));

        // Test case 3: overflow when adding ptr + len
        let data_ptr_overflow = u64::MAX - 5;
        let buf_ptr_overflow = 48u64;
        prepare_guest_buf_descriptor(&host, buf_ptr_overflow, data_ptr_overflow, 10);

        let buffer_overflow = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr_overflow)
                .unwrap()
        };

        let err_overflow = host.read_guest_memory_slice(&buffer_overflow).unwrap_err();
        assert!(matches!(
            err_overflow,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));
    }

    /// Tests that `read_guest_memory_slice_mut` returns an error when the requested memory region
    /// exceeds the bounds of guest memory.
    #[test]
    fn test_read_guest_memory_slice_mut_out_of_bounds() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        // Get the memory size - one page is 64KB (65536 bytes)
        let memory_size = host.borrow_memory().data_size() as u64;

        // Test case 1: ptr + len exceeds memory size
        let data_ptr = memory_size - 10; // 10 bytes before end of memory
        let buf_ptr = 16u64;
        // Request 100 bytes which will exceed memory bounds
        // Note: we're using the same descriptor format but will interpret it as BufferMut
        host.borrow_memory()
            .write(buf_ptr, &data_ptr.to_le_bytes())
            .unwrap();
        host.borrow_memory()
            .write(buf_ptr + 8, &100u64.to_le_bytes())
            .unwrap();

        let buffer = unsafe {
            host.read_guest_memory_typed::<sys::BufferMut<'_>>(buf_ptr)
                .unwrap()
        };

        let err = host.read_guest_memory_slice_mut(&buffer).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));

        // Test case 2: ptr itself is beyond memory size
        let data_ptr_beyond = memory_size + 100;
        let buf_ptr_beyond = 32u64;
        host.borrow_memory()
            .write(buf_ptr_beyond, &data_ptr_beyond.to_le_bytes())
            .unwrap();
        host.borrow_memory()
            .write(buf_ptr_beyond + 8, &10u64.to_le_bytes())
            .unwrap();

        let buffer_beyond = unsafe {
            host.read_guest_memory_typed::<sys::BufferMut<'_>>(buf_ptr_beyond)
                .unwrap()
        };

        let err_beyond = host
            .read_guest_memory_slice_mut(&buffer_beyond)
            .unwrap_err();
        assert!(matches!(
            err_beyond,
            VMLogicError::HostError(HostError::InvalidMemoryAccess)
        ));
    }

    /// Tests that valid memory accesses within bounds succeed.
    #[test]
    fn test_read_guest_memory_slice_valid_bounds() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let host = logic.host_functions(store.as_store_mut());

        // Write data at the beginning of memory
        let test_data = b"test data";
        let data_ptr = 100u64;
        host.borrow_memory().write(data_ptr, test_data).unwrap();

        let buf_ptr = 16u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, test_data.len() as u64);

        let buffer = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(buf_ptr)
                .unwrap()
        };

        let result = host.read_guest_memory_slice(&buffer).unwrap();
        assert_eq!(result, test_data);

        // Test at the edge of memory (valid access)
        let memory_size = host.borrow_memory().data_size() as u64;
        let edge_data = b"edge";
        let edge_ptr = memory_size - edge_data.len() as u64;
        host.borrow_memory().write(edge_ptr, edge_data).unwrap();

        let edge_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(&host, edge_buf_ptr, edge_ptr, edge_data.len() as u64);

        let edge_buffer = unsafe {
            host.read_guest_memory_typed::<sys::Buffer<'_>>(edge_buf_ptr)
                .unwrap()
        };

        let edge_result = host.read_guest_memory_slice(&edge_buffer).unwrap();
        assert_eq!(edge_result, edge_data);
    }
}
