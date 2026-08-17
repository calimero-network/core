use borsh::to_vec;
use core::cell::RefCell;
use core::mem;
use serde::Serialize;
use std::cell::Cell;
use tracing::{debug, error, info, trace, warn};

use crate::store::Storage as RuntimeStorage;
use crate::{
    errors::{HostError, Location, PanicContext},
    logic::{sys, VMHostFunctions, VMLogicError, VMLogicResult},
};
use calimero_primitives::common::DIGEST_SIZE;
use calimero_storage::env::{with_runtime_env, IndexCallbacks, RuntimeEnv};
use calimero_storage::{
    address::Id, entities::Metadata, index::Index, interface::Interface, store::MainStorage,
};
use std::rc::Rc;

/// Construct a `RuntimeEnv` that forwards storage calls from the storage crate
/// back into the VM's current `RuntimeStorage`.
///
/// The storage crate keeps its own thread-local accessors (used by both the WASM
/// stubs and the mock in-process tests). When the JS runtime calls into
/// `Interface::<MainStorage>::save_raw`/`find_by_id_raw` we need those calls to
/// hit the per-execution storage handle (`logic.storage`) rather than the
/// default mock store.  We cannot hand that trait object across the boundary
/// directly, so we expose a set of closures that capture the data and vtable
/// pointers of the current storage instance.  While the runtime call is in
/// flight we install this `RuntimeEnv` via `with_runtime_env`, allowing the
/// storage crate to resolve reads/writes against the live context storage.
pub(super) fn build_runtime_env(
    storage: &mut dyn RuntimeStorage,
    context_id: [u8; DIGEST_SIZE],
    executor_id: [u8; DIGEST_SIZE],
    account_id: [u8; DIGEST_SIZE],
) -> RuntimeEnv {
    // Erase the borrow lifetime of the storage trait object so the callbacks
    // can satisfy `RuntimeEnv`'s `'static` closure bound. Crucially we keep the
    // trait-object pointer *intact* as a single fat pointer rather than
    // splitting it into its (data, vtable) halves: that split relied on the
    // unspecified internal layout of Rust trait-object pointers. `*mut dyn _`
    // is `Copy`, so the fat pointer lives happily inside a `Cell`.
    //
    // SAFETY: the transmute only extends the lifetime of the trait object;
    //         source and target are both `*mut dyn RuntimeStorage` fat pointers
    //         with identical layout. The extended lifetime never actually
    //         outlives `storage`: the pointer is only ever dereferenced while
    //         this `RuntimeEnv` is installed via `with_runtime_env`, which
    //         happens strictly within the host call that created it (see the
    //         per-closure safety notes below).
    // Whether this backend actually persists the ordered index (only the real
    // `ContextStorage` does). Captured via the `&mut` borrow before it is erased
    // into the raw pointer below, so the bridge is installed only when it has a
    // real target — test mocks (`SimpleMockStorage`, which implements just
    // get/set/remove/has) return `false` and keep using `calimero-storage`'s
    // process-thread-local index mock, exactly as before this bridge existed.
    let wants_index = storage.supports_index();

    let raw_ptr: *mut dyn RuntimeStorage = storage;
    let raw_static: *mut (dyn RuntimeStorage + 'static) = unsafe { mem::transmute(raw_ptr) };
    let storage_cell = Rc::new(Cell::new(raw_static));

    let reader_cell = Rc::clone(&storage_cell);
    let reader = Rc::new(move |key: &calimero_storage::store::Key| {
        let ptr = reader_cell.get();
        let key_vec = key.to_bytes().to_vec();
        // SAFETY: see `build_runtime_env`. While the host function is executing
        //         the VM guarantees exclusivity over `logic.storage`, so it is
        //         sound to dereference `ptr` here — the only place this closure
        //         runs.
        unsafe { (&*ptr).get(&key_vec) }
    });

    let writer_cell = Rc::clone(&storage_cell);
    let writer = Rc::new(move |key: calimero_storage::store::Key, value: &[u8]| {
        let ptr = writer_cell.get();
        let key_vec = key.to_bytes().to_vec();
        // SAFETY: as above; exclusive access to `logic.storage` is guaranteed
        //         for the duration of the host call.
        unsafe { (&mut *ptr).set(key_vec, value.to_vec()).is_some() }
    });

    let remover_cell = Rc::clone(&storage_cell);
    let remover = Rc::new(move |key: &calimero_storage::store::Key| {
        let ptr = remover_cell.get();
        let key_vec = key.to_bytes().to_vec();
        // SAFETY: as above; exclusive access to `logic.storage` is guaranteed
        //         for the duration of the host call.
        unsafe { (&mut *ptr).remove(&key_vec).is_some() }
    });

    // Safety notes:
    //
    // * The closures above capture the (lifetime-erased) fat pointer to
    //   `storage`. While the host function is executing the VM guarantees
    //   exclusivity over `logic.storage`, so it is safe to dereference that
    //   pointer inside the closures.
    // * The pointer is stored in a `Cell` to keep the closures `Fn` (instead of
    //   `FnMut`), which matches the storage crate’s expectations.
    // * When the host function returns the `RuntimeEnv` drops out of scope and
    //   the storage crate falls back to its default environment, so subsequent
    //   calls that do not install an override will continue to use the mock /
    //   WASM backends.

    // Both identities travel: native storage code inside this execution gates on
    // the account (`env::account_id`) and stamps with the device
    // (`env::device_id`), exactly as the guest does through the host functions.
    let base = RuntimeEnv::new(reader, writer, remover, context_id, executor_id, account_id);

    // Only bridge the ordered index when the backend actually persists it (the
    // real `ContextStorage`). A backend that doesn't (test mocks) leaves the
    // bridge off, so native `SortedSet`/`SortedMap` index ops fall back to
    // `calimero-storage`'s process-thread-local mock — the pre-bridge behaviour
    // the runtime unit tests rely on.
    if !wants_index {
        return base;
    }

    // Ordered-index bridge: route the node-local ordered index + validity marker
    // to the SAME `ContextStorage` (its `Column::SortedIndex`/`SortedIndexMeta`).
    // Without this, a host-side `SortedSet`/`SortedMap` (the JS SDK path, and
    // native `apply_action`) would hit the storage crate's process-thread-local
    // mock instead of the durable, context-scoped columns — so an ordered read
    // and its sync-apply marker clear could target different stores and a
    // converged set could stay stale on the ordered readers (sdk-js#87). Every
    // closure dereferences the same exclusive `storage` pointer as the callbacks
    // above; the same safety reasoning applies.
    let index = {
        macro_rules! idx_cell {
            () => {{
                Rc::clone(&storage_cell)
            }};
        }
        let set_cell = idx_cell!();
        let remove_cell = idx_cell!();
        let remove_prefix_cell = idx_cell!();
        let scan_cell = idx_cell!();
        let last_cell = idx_cell!();
        let meta_set_cell = idx_cell!();
        let meta_get_cell = idx_cell!();
        let meta_clear_cell = idx_cell!();
        IndexCallbacks {
            // SAFETY (every closure): the VM holds exclusive access to
            // `logic.storage` for the duration of the host call, the only time
            // these run — same invariant as the read/write/remove closures above.
            set: Rc::new(move |key: &[u8], value: &[u8]| unsafe {
                (&mut *set_cell.get()).index_set(key, value)
            }),
            remove: Rc::new(move |key: &[u8]| unsafe { (&mut *remove_cell.get()).index_del(key) }),
            remove_prefix: Rc::new(move |prefix: &[u8]| unsafe {
                (&mut *remove_prefix_cell.get()).index_del_prefix(prefix)
            }),
            scan: Rc::new(
                move |lo: &[u8], hi: &[u8], offset: usize, limit: Option<usize>| unsafe {
                    (&*scan_cell.get()).index_scan(lo, hi, offset, limit)
                },
            ),
            last: Rc::new(move |lo: &[u8], hi: &[u8]| unsafe {
                (&*last_cell.get()).index_last(lo, hi)
            }),
            meta_set: Rc::new(move |key: &[u8], value: &[u8]| unsafe {
                (&mut *meta_set_cell.get()).index_meta_set(key, value)
            }),
            meta_get: Rc::new(move |key: &[u8]| unsafe {
                (&*meta_get_cell.get()).index_meta_get(key)
            }),
            meta_clear: Rc::new(move |key: &[u8]| unsafe {
                (&mut *meta_clear_cell.get()).index_meta_del(key)
            }),
        }
    };

    base.with_index(index)
}

thread_local! {
    /// The name of the callback handler method to call when emitting events with handlers.
    /// This is set temporarily by the SDK's `emit_with_handler` function and read by the runtime.
    ///
    /// The runtime reuses OS threads across executions, so this thread-local must
    /// never be allowed to outlive the execution that set it: a value left behind
    /// by a prior execution would be read by [`VMHostFunctions::emit`] during a
    /// later one and misattribute that event to a handler from a different
    /// context. [`CallbackHandlerGuard`] scopes it to a single execution.
    static CURRENT_CALLBACK_HANDLER: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// RAII guard that scopes [`CURRENT_CALLBACK_HANDLER`] to a single execution.
///
/// Entering clears any value left behind on this thread (stashing whatever was
/// there) and dropping restores it. Because the runtime pools and reuses OS
/// threads, holding this guard for the duration of an execution guarantees that
/// a callback-handler name set while running one context can never leak into a
/// later execution that happens to reuse the same thread. Save-and-restore
/// (rather than unconditionally clearing) also keeps re-entrant executions
/// correct: a nested run restores the outer run's value when it finishes.
///
/// # Usage
///
/// Multiple guards on the same thread must be released in strictly nested
/// (LIFO) order, since each restores the value it observed at `enter()`. Held
/// as ordinary locals — the way `Module::run` uses it — Rust's LIFO drop order
/// guarantees this; restoring guards out of order would clobber the saved
/// value. The value must also actually be held: `#[must_use]` flags the
/// `enter();`-and-discard mistake, which would drop the guard immediately and
/// leave nothing scoped.
#[must_use = "the guard must be held for the duration of the execution"]
pub struct CallbackHandlerGuard {
    previous: Option<String>,
}

impl CallbackHandlerGuard {
    /// Enters a fresh callback-handler scope for the current execution, clearing
    /// (and stashing) any value left behind on this thread.
    pub fn enter() -> Self {
        let previous = CURRENT_CALLBACK_HANDLER.with(|name| name.borrow_mut().take());
        Self { previous }
    }
}

impl Drop for CallbackHandlerGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT_CALLBACK_HANDLER.with(|name| *name.borrow_mut() = previous);
    }
}

/// Represents a structured event emitted during the execution.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct Event {
    /// A string identifying the type or category of the event.
    pub kind: String,
    /// The binary data payload associated with the event.
    pub data: Vec<u8>,
    /// Optional handler name for the event.
    pub handler: Option<String>,
}

/// Represents a cross-context call to be executed.
#[derive(Debug, Serialize)]
#[non_exhaustive]
pub struct XCall {
    /// The context ID to execute the call on.
    pub context_id: [u8; DIGEST_SIZE],
    /// The function name to call.
    pub function: String,
    /// The parameters to pass to the function.
    pub params: Vec<u8>,
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
        // SAFETY: `sys::Location<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let location =
            unsafe { self.read_guest_memory_typed::<sys::Location<'_>>(src_location_ptr)? };

        let file = self.read_guest_memory_str(location.file())?.to_owned();
        let line = location.line();
        let column = location.column();

        warn!(
            target: "runtime::host::system",
            file = %file,
            line,
            column,
            "Guest panic() without message"
        );

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
    ///   the UTF-8 panic message.
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
        debug!(
            target: "runtime::host::system",
            src_panic_msg_ptr,
            src_location_ptr,
            "panic_utf8 invoked"
        );
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let panic_message_buf =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_panic_msg_ptr)? };
        // SAFETY: `sys::Location<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let location =
            unsafe { self.read_guest_memory_typed::<sys::Location<'_>>(src_location_ptr)? };

        let panic_message = self.read_guest_memory_str(&panic_message_buf)?.to_owned();
        let file = self.read_guest_memory_str(location.file())?.to_owned();
        let line = location.line();
        let column = location.column();

        error!(
            target: "runtime::host::system",
            message = %panic_message,
            file = %file,
            line,
            column,
            "Guest panic captured"
        );

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
        let len = self
            .borrow_logic()
            .registers
            .get_len(register_id)
            .unwrap_or(u64::MAX);

        trace!(
            target: "runtime::host::system",
            register_id,
            len,
            "register_len"
        );

        Ok(len)
    }

    /// Reads the data from a register into a guest memory buffer.
    ///
    /// # Arguments
    ///
    /// * `register_id` - The ID of the register to read from.
    /// * `dest_data_ptr` - A pointer in guest memory to a destination buffer `sys::BufferMut`
    ///   where the data should be copied.
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
        // SAFETY: `sys::BufferMut<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let dest_data =
            unsafe { self.read_guest_memory_typed::<sys::BufferMut<'_>>(dest_data_ptr)? };

        let data = self.borrow_logic().registers.get(register_id)?;

        if data.len() != usize::try_from(dest_data.len()).map_err(|_| HostError::IntegerOverflow)? {
            trace!(
                target: "runtime::host::system",
                register_id,
                register_size = data.len(),
                dest_size = dest_data.len(),
                "read_register length mismatch"
            );
            return Ok(0);
        }

        self.write_guest_memory_slice(&dest_data, data)?;

        trace!(
            target: "runtime::host::system",
            register_id,
            bytes_copied = data.len(),
            "read_register"
        );

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
        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            logic
                .registers
                .set(logic.limits, dest_register_id, logic.context.context_id)?;
            Ok(())
        })?;

        trace!(
            target: "runtime::host::system",
            dest_register_id,
            "context_id written"
        );

        Ok(())
    }

    /// Handles QuickJS debug prints routed through `js_std_d_print`.
    ///
    /// QuickJS' libc invokes this host import to surface diagnostics. We treat it like any
    /// other guest log, storing it in the execution outcome and emitting it at `info` level.
    pub fn js_std_d_print(
        &mut self,
        _ctx_ptr: u64,
        message_ptr: u64,
        message_len: u64,
    ) -> VMLogicResult<u32> {
        trace!(
            target: "runtime::guest::log",
            ptr = message_ptr,
            len = message_len,
            "js_std_d_print invoked"
        );

        let len = usize::try_from(message_len).map_err(|_| HostError::IntegerOverflow)?;

        // Bound the guest-provided length against actual guest memory *before*
        // allocating. Sizing `vec![0u8; len]` directly from an unchecked guest
        // length lets the guest force an enormous host allocation (OOM); the
        // read below would reject an out-of-bounds region, but only after the
        // allocation had already happened.
        let bytes = if len == 0 {
            Vec::new()
        } else {
            let ptr = usize::try_from(message_ptr).map_err(|_| HostError::IntegerOverflow)?;
            let memory = self.borrow_memory();
            let memory_size = memory.data_size() as usize;
            let end = ptr.checked_add(len).ok_or(HostError::InvalidMemoryAccess)?;
            if end > memory_size {
                return Err(HostError::InvalidMemoryAccess.into());
            }

            let mut buf = vec![0u8; len];
            memory
                .read(message_ptr, &mut buf)
                .map_err(|_| HostError::InvalidMemoryAccess)?;
            buf
        };

        let message = String::from_utf8_lossy(&bytes).to_string();
        let max_len = {
            let logic = self.borrow_logic();
            if logic.logs.len()
                >= usize::try_from(logic.limits.max_logs).map_err(|_| HostError::IntegerOverflow)?
            {
                return Err(HostError::LogsOverflow.into());
            }
            usize::try_from(logic.limits.max_log_size).map_err(|_| HostError::IntegerOverflow)?
        };
        if message.len() > max_len {
            return Err(HostError::LogLengthOverflow.into());
        }
        self.with_logic_mut(|logic| logic.logs.push(message.clone()));

        // Split by audience. The app's own log line is written by app code over
        // app state, so it can hold anything the app was given — and a node
        // operator is not supposed to see user data. It is already returned to
        // the caller in the execution outcome, which is the audience entitled to
        // it, so the node's log keeps the shape of the line and not its content.
        //
        // The content stays available at `trace`, because reading guest output
        // in a node log is a real way to debug an app. That level is the whole
        // point: it is off by default AND off under a blanket `RUST_LOG=debug`,
        // which is what every e2e node runs and what gets uploaded as a CI
        // artifact. Someone who wants it asks for it by name with
        // `RUST_LOG=runtime::guest::log=trace`.
        let total_logs = self.borrow_logic().logs.len();
        info!(
            target: "runtime::guest::log",
            interesting = false,
            total_logs,
            message_len = message.len(),
            "guest log (js_std_d_print)"
        );
        trace!(
            target: "runtime::guest::log",
            total_logs,
            message = %message,
            "guest log message (js_std_d_print)"
        );

        Ok(0)
    }

    /// Copies the **account** this call is authorized as into a register.
    ///
    /// The id an app keys per-person state by. Several devices of one account
    /// write the same value here, which is the whole point — and the reason it
    /// must never be used where per-writer uniqueness matters. For that, see
    /// [`device_id`](Self::device_id).
    ///
    /// # Arguments
    ///
    /// * `dest_register_id` - The ID of the destination register.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails (e.g., exceeds limits).
    pub fn account_id(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            logic
                .registers
                .set(logic.limits, dest_register_id, logic.context.account_id)
        })?;

        trace!(
            target: "runtime::host::system",
            dest_register_id,
            "account_id written"
        );

        Ok(())
    }

    /// Copies the executing **device**'s public key into a register.
    ///
    /// The replica this node speaks as: unique per installation, and what signs
    /// its writes. Distinct from [`account_id`](Self::account_id) — two devices of
    /// one person differ here and agree there.
    ///
    /// # Arguments
    ///
    /// * `dest_register_id` - The ID of the destination register.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails (e.g., exceeds limits).
    pub fn device_id(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            logic.registers.set(
                logic.limits,
                dest_register_id,
                logic.context.executor_public_key,
            )
        })?;

        trace!(
            target: "runtime::host::system",
            dest_register_id,
            "device_id written"
        );

        Ok(())
    }

    /// `account_id` under the pre-split name, for WASM built before the split.
    ///
    /// **A linking shim, not an API.** `calimero-sys` does not declare it, so no
    /// app compiled against the current SDK can import it and `env::executor_id()`
    /// does not exist — the deletion that forces every new call site to choose
    /// between an account and a device is intact. This exists because the import
    /// name is baked into every already-built blob: dropping it turns a stale
    /// fixture into `Link(Import("env", "executor_id", UnknownImport))` at
    /// instantiation, which is a 500 on the first context creation and looks
    /// nothing like an ABI change.
    ///
    /// **It returns the ACCOUNT.** Before the split one identity served both
    /// roles, so a shim has to choose which of the two a stale blob meant, and
    /// the choice is not symmetric. An app reaching for an identity is doing
    /// ownership: `AuthoredMap`, a writer set, `Map<identity, Vote>`. Handing
    /// those a device is the failure this split exists to end — every key of such
    /// a map silently becomes per-installation, so one person voting from a phone
    /// and a laptop counts twice, and nothing errors.
    ///
    /// The opposite mistake is real but not reachable from here: a CRDT replica
    /// slot does need the device, and giving it an account would collapse two
    /// installations onto one slot. Those slots are seeded inside the storage
    /// layer from the execution's device, never through this import — so no
    /// counter or HLC seed is resolved by calling it.
    ///
    /// Removable outright once every consumer that pins a pre-split blob has
    /// rebuilt; `env::account_id()` is what they should be calling.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails (e.g., exceeds limits).
    pub fn executor_id(&mut self, dest_register_id: u64) -> VMLogicResult<()> {
        self.account_id(dest_register_id)
    }

    /// Writes the xcall origin (the source context id) into `dest_register_id`
    /// and returns `1` when this execution was dispatched via `xcall`. Returns
    /// `0` and leaves the register untouched for a direct/RPC call.
    ///
    /// The origin is set by the node from the calling context — never from
    /// guest memory — so a target may trust it as caller provenance.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if the register operation fails.
    pub fn xcall_origin(&mut self, dest_register_id: u64) -> VMLogicResult<u32> {
        let origin = self.borrow_logic().context.xcall_origin;
        let Some(origin) = origin else {
            return Ok(0);
        };
        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            logic.registers.set(logic.limits, dest_register_id, origin)
        })?;

        trace!(
            target: "runtime::host::system",
            dest_register_id,
            "xcall_origin written"
        );

        Ok(1)
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
        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            logic
                .registers
                .set(logic.limits, dest_register_id, &*logic.context.input)
        })?;

        trace!(
            target: "runtime::host::system",
            dest_register_id,
            input_len = self.borrow_logic().context.input.len(),
            "input copied to register"
        );

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
    ///   which is an enum indicating success or error, along with the data buffer.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn value_return(&mut self, src_value_ptr: u64) -> VMLogicResult<()> {
        // `sys::ValueReturn` is a `#[repr(C, u64)]` enum: an 8-byte discriminant
        // followed by a `Buffer` payload. The discriminant comes straight from
        // guest memory, so reinterpreting the whole enum with `assume_init`
        // would be undefined behaviour if the guest supplied an out-of-range
        // tag. Read and validate the discriminant as a plain `u64` first, then
        // read the `Buffer` payload separately — this never materializes a
        // `ValueReturn` with an invalid discriminant.
        let mut discriminant_bytes = [0u8; mem::size_of::<u64>()];
        self.borrow_memory()
            .read(src_value_ptr, &mut discriminant_bytes)?;
        let discriminant = u64::from_le_bytes(discriminant_bytes);

        // The `Buffer` payload follows the 8-byte discriminant.
        let payload_ptr = src_value_ptr
            .checked_add(mem::size_of::<u64>() as u64)
            .ok_or(HostError::InvalidMemoryAccess)?;
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the read is bounds-checked. See `read_guest_memory_typed`.
        let value = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(payload_ptr)? };

        // Bound the return value before copying it out of guest memory: it lands
        // on the host `Outcome` (and is broadcast in receipts), so without this
        // cap the only limit is guest memory itself (~64 MiB).
        let value_len = value.len();
        let max_return_value_size = self.borrow_logic().limits.max_return_value_size;
        if value_len > max_return_value_size {
            return Err(HostError::ReturnValueSizeOverflow {
                size: value_len,
                max: max_return_value_size,
            }
            .into());
        }

        let bytes = self.read_guest_memory_slice(&value)?.to_vec();

        // Discriminant layout matches `sys::ValueReturn`: 0 = Ok, 1 = Err.
        let result = match discriminant {
            0 => Ok(bytes),
            1 => Err(bytes),
            other => {
                warn!(
                    target: "runtime::host::system",
                    discriminant = other,
                    "value_return got an out-of-range ValueReturn discriminant"
                );
                return Err(HostError::DeserializationError.into());
            }
        };

        let result_len = match &result {
            Ok(value) | Err(value) => value.len(),
        };
        let was_ok = result.is_ok();

        self.with_logic_mut(|logic| logic.returns = Some(result));

        debug!(
            target: "runtime::host::system",
            success = was_ok,
            bytes = result_len,
            "value_return captured"
        );

        Ok(())
    }

    /// Captures the transient migration witness emitted by `#[app::migrate]`.
    ///
    /// The witness is a borsh blob that `#[app::migration_check]` reads via the
    /// repacked check input. It rides out on `Outcome` like logs/events and is
    /// NEVER written to storage; bounded by `max_storage_value_size`.
    ///
    /// # Arguments
    ///
    /// * `src_ptr` - A pointer in guest memory to a source-`sys::Buffer` with the witness bytes.
    ///
    /// # Errors
    ///
    /// * `HostError::ValueLengthOverflow` if the witness exceeds the value-size limit.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for the buffer descriptor.
    pub fn emit_migration_witness(&mut self, src_ptr: u64) -> VMLogicResult<()> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let src_buf = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_ptr)? };
        let bytes = self.read_guest_memory_slice(&src_buf)?.to_vec();

        let max_len = usize::try_from(self.borrow_logic().limits.max_storage_value_size.get())
            .map_err(|_| HostError::IntegerOverflow)?;
        if bytes.len() > max_len {
            return Err(HostError::ValueLengthOverflow.into());
        }

        let witness_len = bytes.len();
        self.with_logic_mut(|logic| logic.migration_witness = Some(bytes));

        debug!(
            target: "runtime::host::system",
            bytes = witness_len,
            "migration witness captured"
        );

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
        trace!(
            target: "runtime::guest::log",
            ptr = src_log_ptr,
            "log_utf8 invoked"
        );

        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let src_log_buf =
            match unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_log_ptr) } {
                Ok(buf) => buf,
                Err(err) => {
                    error!(
                        target: "runtime::guest::log",
                        ptr = src_log_ptr,
                        error = ?err,
                        "failed to read guest log buffer descriptor"
                    );
                    return Err(err);
                }
            };

        let message = match self.read_guest_memory_str(&src_log_buf) {
            Ok(msg) => msg.to_owned(),
            Err(err) => {
                error!(
                    target: "runtime::guest::log",
                    ptr = src_log_ptr,
                    buf_len = src_log_buf.len(),
                    error = ?err,
                    "failed to read guest log message"
                );
                return Err(err);
            }
        };
        let max_len = {
            let logic = self.borrow_logic();
            if logic.logs.len()
                >= usize::try_from(logic.limits.max_logs).map_err(|_| HostError::IntegerOverflow)?
            {
                return Err(HostError::LogsOverflow.into());
            }
            usize::try_from(logic.limits.max_log_size).map_err(|_| HostError::IntegerOverflow)?
        };
        if message.len() > max_len {
            return Err(HostError::LogLengthOverflow.into());
        }

        self.with_logic_mut(|logic| logic.logs.push(message.clone()));

        let total_logs = self.borrow_logic().logs.len();
        let interesting = message.contains("[dispatcher]") || message.contains("QuickJS");

        // As in `js_std_d_print`: shape at info, content at trace. `interesting`
        // still says whether this was a dispatcher/QuickJS line, which is what
        // the flag was for and needs no user data to answer.
        info!(
            target: "runtime::guest::log",
            interesting,
            total_logs,
            message_len = message.len(),
            "guest log"
        );
        trace!(
            target: "runtime::guest::log",
            interesting,
            total_logs,
            message = %message,
            "guest log message"
        );

        Ok(())
    }

    /// Emits a structured event that is added to the events log.
    ///
    /// Events are recorded and included in the final execution `Outcome`.
    ///
    /// # Arguments
    ///
    /// * `src_event_ptr` - A pointer in guest memory to a `sys::Event` struct, which
    ///   contains source-buffers for the event `kind` and `data`.
    ///
    /// # Errors
    ///
    /// * `HostError::EventKindSizeOverflow` if the event kind is too long.
    /// * `HostError::EventDataSizeOverflow` if the event data is too large.
    /// * `HostError::EventsOverflow` if the maximum number of events has been reached.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn emit(&mut self, src_event_ptr: u64) -> VMLogicResult<()> {
        // SAFETY: `sys::Event<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
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
        let data = self.read_guest_memory_slice(event.data())?.to_vec();

        // Read callback handler name from thread-local storage
        let handler = CURRENT_CALLBACK_HANDLER.with(|name| name.borrow().clone());

        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            logic.events.push(Event {
                kind,
                data,
                handler,
            });
            Ok(())
        })?;

        debug!(
            target: "runtime::host::system",
            events = self.borrow_logic().events.len(),
            kind_len,
            data_len,
            "emit"
        );

        Ok(())
    }

    /// Emits an event with an optional handler name.
    ///
    /// This function is similar to `emit` but includes handler information.
    /// The handler name is read from the provided memory pointer.
    ///
    /// # Arguments
    ///
    /// * `src_event_ptr` - Pointer to the event data in guest memory.
    /// * `src_handler_ptr` - Pointer to the handler name in guest memory (can be 0 for no handler).
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the event was successfully emitted.
    ///
    /// # Errors
    ///
    /// * `HostError::EventKindSizeOverflow` if the event kind is too long.
    /// * `HostError::EventDataSizeOverflow` if the event data is too large.
    /// * `HostError::EventsOverflow` if the maximum number of events has been reached.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn emit_with_handler(
        &mut self,
        src_event_ptr: u64,
        src_handler_ptr: u64,
    ) -> VMLogicResult<()> {
        // SAFETY: `sys::Event<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
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
        let data = self.read_guest_memory_slice(event.data())?.to_vec();

        // Parse handler name if provided (src_handler_ptr != 0)
        let handler = if src_handler_ptr == 0 {
            None
        } else {
            // Read the handler buffer from guest memory
            // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
            //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
            //         it is sound; the guest SDK wrote a well-formed instance at this
            //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
            let handler_buffer =
                unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_handler_ptr)? };
            // A handler is a callback method name; bound it before copying it out
            // of guest memory so a guest can't stash arbitrarily large strings on
            // the `Outcome` (one per event, up to `max_events`). Reuse the
            // method-name limit — that's exactly what a handler is.
            let handler_len = handler_buffer.len();
            let max_handler_size = self.borrow_logic().limits.max_method_name_length;
            if handler_len > max_handler_size {
                return Err(HostError::EventHandlerSizeOverflow {
                    size: handler_len,
                    max: max_handler_size,
                }
                .into());
            }
            // Propagate a read/UTF-8 failure rather than silently dropping the
            // handler — a malformed handler descriptor is a guest bug, and this
            // matches how `emit` reads the event `kind`.
            let handler_str = self.read_guest_memory_str(&handler_buffer)?;
            Some(handler_str.to_owned())
        };

        self.with_logic_mut(|logic| {
            logic.events.push(Event {
                kind,
                data,
                handler,
            });
        });

        Ok(())
    }

    /// Queues a cross-context call to be executed after the current execution completes.
    ///
    /// This function collects cross-context calls that will be executed locally
    /// on the specified contexts after the current execution finishes.
    ///
    /// # Arguments
    ///
    /// * `src_xcall_ptr` - Pointer to the XCall data in guest memory.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the xcall was successfully queued.
    ///
    /// # Errors
    ///
    /// * `HostError::XCallFunctionSizeOverflow` if the function name is too long.
    /// * `HostError::XCallParamsSizeOverflow` if the params data is too large.
    /// * `HostError::XCallsOverflow` if the maximum number of xcalls has been reached.
    /// * `HostError::InvalidMemoryAccess` if memory access fails for descriptor buffers.
    pub fn xcall(&mut self, src_xcall_ptr: u64) -> VMLogicResult<()> {
        // SAFETY: `sys::XCall<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let xcall = unsafe { self.read_guest_memory_typed::<sys::XCall<'_>>(src_xcall_ptr)? };

        let function_len = xcall.function().len();
        let params_len = xcall.params().len();

        let logic = self.borrow_logic();

        if function_len > logic.limits.max_xcall_function_size {
            return Err(HostError::XCallFunctionSizeOverflow.into());
        }

        if params_len > logic.limits.max_xcall_params_size {
            return Err(HostError::XCallParamsSizeOverflow.into());
        }

        if logic.xcalls.len()
            >= usize::try_from(logic.limits.max_xcalls).map_err(|_| HostError::IntegerOverflow)?
        {
            return Err(HostError::XCallsOverflow.into());
        }

        let context_id = *self.read_guest_memory_sized::<DIGEST_SIZE>(xcall.context_id())?;
        let function = self.read_guest_memory_str(xcall.function())?.to_owned();
        let params = self.read_guest_memory_slice(xcall.params())?.to_vec();

        self.with_logic_mut(|logic| {
            logic.xcalls.push(XCall {
                context_id,
                function,
                params,
            });
        });

        Ok(())
    }

    /// Commits the execution state, providing a state root and an artifact.
    ///
    /// Every JS contract must call this exactly once per execution. The runtime
    /// stores the root hash and artifact in the `Outcome`; the **Rust core**
    /// (particularly the Wasm execution services in `crates/context` and
    /// `crates/node`) expects those fields to be present so it can broadcast
    /// receipts, trigger event handlers, and persist execution metadata. This is
    /// the same contract followed by the Rust SDK—those services never inspect
    /// guest memory directly, they rely on the `Outcome` produced here. The
    /// newer `persist_root_state` API complements this by ensuring the Merkle
    /// tree reflects the same state.
    ///
    /// # Arguments
    ///
    /// * `src_root_hash_ptr` - A pointer to a source-buffer in guest memory containing the 32-byte state root hash.
    /// * `src_artifact_ptr` - A pointer to a source-buffer in guest memory containing a binary artifact.
    ///
    /// # Errors
    ///
    /// * `HostError::InvalidMemoryAccess` if this function is called more than once or if memory
    ///   access fails for descriptor buffers.
    /// * `HostError::ArtifactSizeOverflow` if the artifact exceeds `max_artifact_size`.
    pub fn commit(&mut self, src_root_hash_ptr: u64, src_artifact_ptr: u64) -> VMLogicResult<()> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let root_hash =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_root_hash_ptr)? };
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let artifact =
            unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_artifact_ptr)? };

        // Bound the artifact before copying it out of guest memory: the copy
        // lands on the host `Outcome`, so without this cap the only limit is
        // guest memory itself (~64 MiB).
        let max_artifact_size = self.borrow_logic().limits.max_artifact_size;
        if artifact.len() > max_artifact_size {
            return Err(HostError::ArtifactSizeOverflow {
                size: artifact.len(),
                max: max_artifact_size,
            }
            .into());
        }

        let root_hash = *self.read_guest_memory_sized::<DIGEST_SIZE>(&root_hash)?;
        let artifact = self.read_guest_memory_slice(&artifact)?.to_vec();

        self.with_logic_mut(|logic| {
            if logic.commit_called {
                return Err(HostError::InvalidMemoryAccess);
            }

            logic.root_hash = Some(root_hash);
            logic.artifact = artifact;
            logic.commit_called = true;

            Ok(())
        })?;

        Ok(())
    }

    /// Opts the JS app's opaque root into the WASM merge sync path.
    ///
    /// A JS root is not a `#[app::state]` type, so core has no registered
    /// `Mergeable` and would treat it as opaque (`crdt_type: None`), resolving
    /// conflicts by Last-Writer-Wins — which cannot converge concurrent writers.
    /// The JS SDK calls this from its `__calimero_register_merge` hook (invoked by
    /// the runtime in this same execution, before the method runs) to declare that
    /// its module exports `__calimero_merge_root_state`. `persist_root_state` then
    /// stamps the root with the `JsRoot` marker so the sync apply path defers it to
    /// that guest merge callback instead of LWW. Idempotent and non-failing.
    pub fn register_js_sdk_root_merge(&mut self) -> VMLogicResult<()> {
        self.with_logic_mut(|logic| {
            logic.js_root_merge = true;
        });
        Ok(())
    }

    /// Persists the root state document provided by the guest runtime.
    ///
    /// Instead of writing directly to storage (which would bypass Merkle bookkeeping),
    /// the payload is stored through the storage interface so that parent hashes are
    /// recomputed and a CRDT action is emitted. The Rust SDK accomplishes the same thing
    /// by calling storage APIs directly; the JS SDK cannot, so it funnels the serialized
    /// root document back to the host via this hook. Paired with `commit` and
    /// `flush_delta`, this keeps both the outcome artifact and the storage DAG in sync.
    pub fn persist_root_state(
        &mut self,
        src_doc_ptr: u64,
        created_at: u64,
        updated_at: u64,
    ) -> VMLogicResult<()> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_doc_ptr)? };
        let payload = self.read_guest_memory_slice(&buffer)?.to_vec();

        fn hlc_to_nanos(ts: calimero_storage::logical_clock::HybridTimestamp) -> u64 {
            let ntp = ts.get_time().as_u64();
            let secs = ntp >> 32;
            let frac = ntp & 0xFFFF_FFFF;
            let nanos = (frac.saturating_mul(1_000_000_000)) >> 32;
            secs.saturating_mul(1_000_000_000).saturating_add(nanos)
        }

        let logical_time = calimero_storage::env::hlc_timestamp();
        let host_updated_at = hlc_to_nanos(logical_time);

        let base_created_at = if created_at == 0 {
            host_updated_at
        } else {
            created_at
        };

        let previous_updated = updated_at.max(base_created_at);
        let monotonic_updated = host_updated_at.max(previous_updated.saturating_add(1));

        let final_created_at = base_created_at;
        let final_updated_at = monotonic_updated;
        let mut payload_opt = Some(payload);

        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            debug!(
                target: "runtime::host::system",
                "apply_storage_delta using context id"
            );
            let env = build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
                logic.context.account_id,
            );

            let payload = payload_opt
                .take()
                .expect("persist_root_state payload already consumed");

            // A guest that called `register_js_sdk_root_merge` provides a
            // `__calimero_merge_root_state` callback; stamp the root with the
            // `JsRoot` marker so the sync apply path defers to that callback
            // instead of LWW-collapsing concurrent writes. Without the opt-in
            // the root stays opaque (`crdt_type: None`) exactly as before.
            let mut metadata = Metadata::new(final_created_at, final_updated_at);
            if logic.js_root_merge {
                metadata.crdt_type = Some(calimero_primitives::crdt::CrdtType::js_root());
            }

            with_runtime_env(env, move || {
                // Store the root document as the ROOT_ENTRY_ID leaf (a child of
                // Id::root()), NOT on Id::root() itself. A JS root converges only
                // via the HashComparison deferred-merge path, which fires only for
                // leaf entries; on Id::root() (an internal Merkle node once the app
                // owns CRDT collections) it was never deferred. See
                // Interface::save_root_entry.
                Interface::<MainStorage>::save_root_entry(payload, metadata)
            })
            .map_err(|err| {
                VMLogicError::from(HostError::Panic {
                    context: PanicContext::Host,
                    message: format!("persist_root_state failed: {err}"),
                    location: Location::Unknown,
                })
            })?;

            Ok(())
        })?;

        Ok(())
    }

    /// Reads the persisted root state document into a register.
    ///
    /// Returns `1` if the state exists, `0` otherwise.
    pub fn read_root_state(&mut self, dest_register_id: u64) -> VMLogicResult<i32> {
        self.with_logic_mut(|logic| -> VMLogicResult<i32> {
            let context_hex: String = logic
                .context
                .context_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let env = build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
                logic.context.account_id,
            );

            let maybe_bytes = with_runtime_env(env, Interface::<MainStorage>::read_root_entry);

            if let Some(bytes) = maybe_bytes {
                let value_len = bytes.len();
                logic.registers.set(logic.limits, dest_register_id, bytes)?;

                info!(
                    target: "runtime::host::system",
                    value_len,
                    dest_register_id,
                    context_id = %context_hex,
                    "read_root_state returned payload"
                );

                Ok(1)
            } else {
                info!(
                    target: "runtime::host::system",
                    context_id = %context_hex,
                    "read_root_state returned no payload"
                );
                Ok(0)
            }
        })
    }

    /// Applies a serialized storage delta produced by another executor.
    ///
    /// The delta must be encoded as `StorageDelta::Actions` in Borsh format. The host
    /// will deserialize the actions and feed them into the storage interface so that
    /// CRDT entities and the root document are updated atomically.
    pub fn apply_storage_delta(&mut self, src_delta_ptr: u64) -> VMLogicResult<()> {
        // SAFETY: `sys::Buffer<'_>` is a vetted `GuestAbiType` ABI descriptor (a `#[repr(C)]`
        //         layout of `u64`-shaped fields), so reinterpreting the guest bytes as
        //         it is sound; the guest SDK wrote a well-formed instance at this
        //         offset and the read is bounds-checked. See `read_guest_memory_typed`.
        let buffer = unsafe { self.read_guest_memory_typed::<sys::Buffer<'_>>(src_delta_ptr)? };
        let payload = self.read_guest_memory_slice(&buffer)?.to_vec();
        let delta_len = payload.len();

        self.with_logic_mut(|logic| -> VMLogicResult<()> {
            let context_hex: String = logic
                .context
                .context_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            info!(
                target: "runtime::host::system",
                delta_len,
                context_id = %context_hex,
                "apply_storage_delta start"
            );

            let env = build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
                logic.context.account_id,
            );

            with_runtime_env(env.clone(), || {
                // #2266: empty ctx is the TEMPLATE here. `payload` is a
                // pre-built `StorageDelta` artifact — when its variant is
                // `CausalActions`, `Root::sync` builds per-action ctxs
                // from the embedded `effective_writers` map and ignores
                // this template. For `Actions` (host-side replay of
                // already-verified state) the template is used as-is and
                // the verifier falls back to v2 stored-writers, which is
                // safe for replicated state from a peer.
                let sync_ctx = calimero_storage::interface::ApplyContext::empty();
                calimero_storage::collections::Root::<Vec<u8>>::sync(&payload, &sync_ctx)
            })
            .map_err(|err| {
                VMLogicError::from(HostError::Panic {
                    context: PanicContext::Host,
                    message: format!("apply_storage_delta failed: {err}"),
                    location: Location::Unknown,
                })
            })?;

            let root_hash =
                with_runtime_env(env, || Index::<MainStorage>::get_hashes_for(Id::root()))
                    .map_err(|err| {
                        VMLogicError::from(HostError::Panic {
                            context: PanicContext::Host,
                            message: format!(
                                "apply_storage_delta failed to fetch root hash: {err}"
                            ),
                            location: Location::Unknown,
                        })
                    })?
                    .map(|(full_hash, _)| full_hash)
                    .unwrap_or([0; 32]);

            logic.root_hash = Some(root_hash);

            info!(
                target: "runtime::host::system",
                delta_len,
                context_id = %context_hex,
                "apply_storage_delta completed"
            );

            Ok(())
        })?;

        Ok(())
    }

    /// Flushes pending CRDT actions recorded by the storage layer and commits them
    /// as a causal delta.
    ///
    /// The delta is only emitted if `persist_root_state` (or other storage
    /// operations) recorded actions since the last flush. The Rust SDK triggers
    /// this automatically when collections mutate; the JS SDK does it manually.
    /// Returns `1` if a delta was emitted, `0` if there was nothing to commit.
    pub fn flush_delta(&mut self) -> VMLogicResult<i32> {
        self.with_logic_mut(|logic| -> VMLogicResult<i32> {
            let env = build_runtime_env(
                logic.storage,
                logic.context.context_id,
                logic.context.executor_public_key,
                logic.context.account_id,
            );

            let root_hash = with_runtime_env(env.clone(), || {
                Index::<MainStorage>::get_hashes_for(Id::root())
            })
            .map_err(|err| HostError::Panic {
                context: PanicContext::Host,
                message: format!("failed to fetch root hash: {err}"),
                location: Location::Unknown,
            })?
            .map(|(full_hash, _)| full_hash)
            .unwrap_or([0; 32]);

            let commit_result = with_runtime_env(env, || {
                calimero_storage::delta::commit_causal_delta(&root_hash)
            })
            .map_err(|err| {
                VMLogicError::from(HostError::Panic {
                    context: PanicContext::Host,
                    message: format!("commit_causal_delta failed: {err}"),
                    location: Location::Unknown,
                })
            })?;

            match commit_result {
                Some(delta) => {
                    use calimero_storage::interface::Action;

                    let action_ids: Vec<String> = delta
                        .actions
                        .iter()
                        .map(|action| match action {
                            Action::Add { id, .. }
                            | Action::Update { id, .. }
                            | Action::DeleteRef { id, .. } => format!("{id:?}"),
                        })
                        .collect();

                    debug!(
                        target: "runtime::host::system",
                        action_count = delta.actions.len(),
                        parent_count = delta.parents.len(),
                        action_ids = ?action_ids,
                        "flush_delta emitting causal delta"
                    );
                    let storage_delta =
                        calimero_storage::delta::StorageDelta::Actions(delta.actions.clone());
                    let artifact = to_vec(&storage_delta).map_err(|err| {
                        VMLogicError::from(HostError::Panic {
                            context: PanicContext::Host,
                            message: format!("failed to serialize causal delta: {err}"),
                            location: Location::Unknown,
                        })
                    })?;

                    logic.root_hash = Some(root_hash);
                    logic.artifact = artifact;
                    Ok(1)
                }
                None => {
                    logic.root_hash = Some(root_hash);
                    Ok(0)
                }
            }
        })
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

    use super::{CallbackHandlerGuard, CURRENT_CALLBACK_HANDLER};

    fn current_handler() -> Option<String> {
        CURRENT_CALLBACK_HANDLER.with(|name| name.borrow().clone())
    }

    /// The guard clears any value left behind on the thread when entered, so an
    /// execution always starts with a fresh (empty) callback handler.
    #[test]
    fn test_callback_handler_guard_clears_stale_value_on_enter() {
        // Simulate a value leaked from a "previous execution" on this thread.
        CURRENT_CALLBACK_HANDLER.with(|name| *name.borrow_mut() = Some("stale".to_owned()));

        {
            let _scope = CallbackHandlerGuard::enter();
            assert_eq!(
                current_handler(),
                None,
                "entering a scope must clear a value left by a prior execution"
            );
        }
    }

    /// The guard restores the previous value on drop, so a re-entrant execution
    /// does not clobber the outer execution's callback handler.
    #[test]
    fn test_callback_handler_guard_restores_previous_on_drop() {
        let outer = CallbackHandlerGuard::enter();
        CURRENT_CALLBACK_HANDLER.with(|name| *name.borrow_mut() = Some("outer".to_owned()));

        {
            // A nested execution enters its own scope...
            let _inner = CallbackHandlerGuard::enter();
            assert_eq!(current_handler(), None, "nested scope starts empty");
            CURRENT_CALLBACK_HANDLER.with(|name| *name.borrow_mut() = Some("inner".to_owned()));
        }

        // ...and on drop the outer execution's value is back in place.
        assert_eq!(
            current_handler(),
            Some("outer".to_owned()),
            "dropping the nested scope must restore the outer value"
        );

        drop(outer);
        assert_eq!(
            current_handler(),
            None,
            "dropping the outermost scope must leave the thread-local empty"
        );
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

    /// Tests the `context_id()`, `account_id()` and `device_id()` host functions.
    ///
    /// The account and the device are given **different** bytes on purpose. Both
    /// are 32-byte values reached through the same register mechanism, so a wiring
    /// mistake that served one where the other was asked for is invisible to a
    /// fixture that gives them the same value — and serving the device as the
    /// account is precisely the conflation this split exists to end.
    #[test]
    fn test_context_account_and_device_id() {
        let context_id = [3u8; DIGEST_SIZE];
        let device_id = [5u8; DIGEST_SIZE];
        let account = calimero_account::AccountId::from([7u8; DIGEST_SIZE]);
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let context = VMContext::new(Cow::Owned(vec![]), context_id, device_id, account);
        let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);

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

        let device_id_register = 2;
        host.device_id(device_id_register).unwrap();
        assert_eq!(
            host.borrow_logic()
                .registers
                .get(device_id_register)
                .unwrap(),
            device_id,
        );

        let account_id_register = 3;
        host.account_id(account_id_register).unwrap();
        assert_eq!(
            host.borrow_logic()
                .registers
                .get(account_id_register)
                .unwrap(),
            *account.as_bytes(),
            "the account register must carry the account, not the device"
        );
    }

    /// **`executor_id` resolves to the ACCOUNT, not the device.**
    ///
    /// The pre-split name served one identity that later became two, so this shim
    /// has to choose which a stale blob meant. An app reaching for an identity is
    /// doing ownership — `AuthoredMap`, a writer set, `Map<identity, Vote>` — and
    /// giving those a device makes every key per-installation, so one person on a
    /// phone and a laptop counts twice and nothing errors.
    ///
    /// The device and the account are deliberately different values here. If they
    /// were equal this would pass whichever the shim returned.
    #[test]
    fn executor_id_resolves_to_the_account_not_the_device() {
        let context_id = [3u8; DIGEST_SIZE];
        let device = [5u8; DIGEST_SIZE];
        let account = calimero_account::AccountId::from([7u8; DIGEST_SIZE]);
        assert_ne!(
            &device,
            account.as_bytes(),
            "precondition: the two must differ, or this proves nothing"
        );

        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let context = VMContext::new(Cow::Owned(vec![]), context_id, device, account);
        let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
        let mut store = Store::default();
        let memory =
            wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
        let _ = logic.with_memory(memory);
        let mut host = logic.host_functions(store.as_store_mut());

        host.executor_id(1).unwrap();
        assert_eq!(
            host.borrow_logic().registers.get(1).unwrap(),
            account.as_bytes(),
            "a pre-split blob asking for an identity must get the account"
        );

        // And `device_id` still answers with the device, so the split is intact
        // rather than both names having collapsed onto one value.
        host.device_id(2).unwrap();
        assert_eq!(
            host.borrow_logic().registers.get(2).unwrap(),
            &device,
            "device_id must still be the device"
        );
    }

    /// `xcall_origin()` returns 0 and leaves the register untouched for a direct
    /// call (no origin), and returns 1 writing the source context for an xcall.
    #[test]
    fn test_xcall_origin_present_and_absent() {
        let context_id = [3u8; DIGEST_SIZE];
        let executor_id = [5u8; DIGEST_SIZE];

        // Absent: a plain direct/RPC call has no xcall origin.
        {
            let mut storage = SimpleMockStorage::new();
            let limits = VMLimits::default();
            let context = VMContext::new(
                Cow::Owned(vec![]),
                context_id,
                executor_id,
                calimero_account::AccountId::from([7u8; DIGEST_SIZE]),
            );
            let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
            let mut store = Store::default();
            let memory =
                wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
            let _ = logic.with_memory(memory);
            let mut host = logic.host_functions(store.as_store_mut());

            let reg = 4;
            let present = host.xcall_origin(reg).unwrap();
            assert_eq!(present, 0, "no origin ⇒ returns 0");
            assert!(
                host.borrow_logic().registers.get(reg).is_err(),
                "register must be untouched when there is no origin"
            );
        }

        // Present: an xcall-dispatched execution carries the source context.
        {
            let origin = [9u8; DIGEST_SIZE];
            let mut storage = SimpleMockStorage::new();
            let limits = VMLimits::default();
            let mut context = VMContext::new(
                Cow::Owned(vec![]),
                context_id,
                executor_id,
                calimero_account::AccountId::from([7u8; DIGEST_SIZE]),
            );
            context.xcall_origin = Some(origin);
            let mut logic = VMLogic::new(&mut storage, None, context, &limits, None);
            let mut store = Store::default();
            let memory =
                wasmer::Memory::new(&mut store, wasmer::MemoryType::new(1, None, false)).unwrap();
            let _ = logic.with_memory(memory);
            let mut host = logic.host_functions(store.as_store_mut());

            let reg = 5;
            let present = host.xcall_origin(reg).unwrap();
            assert_eq!(present, 1, "origin present ⇒ returns 1");
            assert_eq!(host.borrow_logic().registers.get(reg).unwrap(), origin);
        }
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

    /// A guest that supplies an out-of-range `ValueReturn` discriminant is
    /// rejected with a clean `DeserializationError` rather than being
    /// reinterpreted. The discriminant is read and validated as a plain `u64`
    /// before the enum payload is ever materialized, so an invalid tag can never
    /// drive an `assume_init` on an uninitialized variant (run under Miri to
    /// confirm the absence of UB).
    #[test]
    fn test_value_return_rejects_invalid_discriminant() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // A fully valid payload buffer — the rejection must come from the
        // discriminant alone, not from a malformed buffer descriptor.
        let value = "payload for an invalid discriminant";
        let value_ptr = 200u64;
        write_str(&host, value_ptr, value);

        // Neither 0 (Ok) nor 1 (Err): the only two legal discriminants.
        for bad_discriminant in [2u8, 42u8, 255u8] {
            let return_ptr = 32u64;
            // Clear the 8 discriminant bytes, then write the bad tag.
            host.borrow_memory().write(return_ptr, &[0u8; 8]).unwrap();
            host.borrow_memory()
                .write(return_ptr, &[bad_discriminant])
                .unwrap();
            prepare_guest_buf_descriptor(&host, return_ptr + 8, value_ptr, value.len() as u64);

            let err = host.value_return(return_ptr).unwrap_err();
            assert!(
                matches!(err, VMLogicError::HostError(HostError::DeserializationError)),
                "discriminant {bad_discriminant} must be rejected with DeserializationError, got {err:?}"
            );
            // Nothing must have been captured as the execution's return value.
            assert!(
                host.borrow_logic().returns.is_none(),
                "an invalid discriminant must not capture a return value"
            );
        }
    }

    /// `emit_migration_witness` captures the guest blob into the transient
    /// migrate→check channel on VMLogic (never via storage).
    #[test]
    fn test_emit_migration_witness() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let witness = [1u8, 2, 3, 4];
        let data_ptr = 200u64;
        // Guest: write the witness bytes and a descriptor pointing at them.
        host.borrow_memory().write(data_ptr, &witness).unwrap();
        let buf_ptr = 10u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, data_ptr, witness.len() as u64);

        host.emit_migration_witness(buf_ptr)
            .expect("witness emit failed");
        assert_eq!(
            host.borrow_logic().migration_witness.as_deref(),
            Some(&witness[..])
        );
    }

    /// A captured witness rides out on the Outcome (like logs/events).
    #[test]
    fn test_finish_surfaces_migration_witness() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, _store) = setup_vm!(&mut storage, &limits, vec![]);
        logic.migration_witness = Some(vec![9, 9, 9]);
        assert_eq!(logic.finish(None).migration_witness, Some(vec![9, 9, 9]));
    }

    /// A run that emits no witness yields `None` on the Outcome.
    #[test]
    fn test_finish_without_witness_is_none() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (logic, _store) = setup_vm!(&mut storage, &limits, vec![]);
        assert_eq!(logic.finish(None).migration_witness, None);
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
        let limits = VMLimits {
            max_logs: 5,
            ..Default::default()
        };
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

    #[test]
    fn test_log_utf8_length_overflow() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits {
            max_log_size: 4,
            ..Default::default()
        };
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "exceeds";
        let msg_ptr = 200u64;
        write_str(&host, msg_ptr, msg);
        let buf_ptr = 12u64;
        prepare_guest_buf_descriptor(&host, buf_ptr, msg_ptr, msg.len() as u64);

        let err = host.log_utf8(buf_ptr).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::LogLengthOverflow)
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

    #[test]
    fn test_js_std_d_print_length_overflow() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits {
            max_log_size: 5,
            ..Default::default()
        };
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg = "too long";
        let msg_ptr = 512u64;
        write_str(&host, msg_ptr, msg);

        let err = host
            .js_std_d_print(0, msg_ptr, msg.len() as u64)
            .unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::LogLengthOverflow)
        ));
    }

    /// A guest that hands `js_std_d_print` an enormous `message_len` must be
    /// rejected by the memory-bounds check *before* any host allocation. Sizing
    /// `vec![0u8; len]` straight from an unchecked guest length would let the
    /// guest force a multi-gigabyte allocation (OOM) purely by lying about the
    /// length; the `ptr + len` overflow / out-of-bounds guard catches it first,
    /// returning `InvalidMemoryAccess` with no large allocation and no panic.
    #[test]
    fn test_js_std_d_print_huge_length_does_not_allocate() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let msg_ptr = 512u64;

        // u64::MAX length: `ptr + len` overflows the address space, so the
        // bounds check fails before `vec![0u8; len]` can run.
        let err = host.js_std_d_print(0, msg_ptr, u64::MAX).unwrap_err();
        assert!(
            matches!(err, VMLogicError::HostError(HostError::InvalidMemoryAccess)),
            "u64::MAX length must be rejected with InvalidMemoryAccess, got {err:?}"
        );

        // A length that does not overflow arithmetically but still runs past the
        // end of guest memory must also be rejected pre-allocation.
        let past_end = host.borrow_memory().data_size() + 1;
        let err = host.js_std_d_print(0, msg_ptr, past_end).unwrap_err();
        assert!(
            matches!(err, VMLogicError::HostError(HostError::InvalidMemoryAccess)),
            "a length past the end of memory must be rejected with InvalidMemoryAccess, got {err:?}"
        );

        // No log was recorded on either rejection.
        assert!(
            host.borrow_logic().logs.is_empty(),
            "a rejected oversized print must not record a log"
        );
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

    /// Tests that `commit()` rejects an artifact larger than `max_artifact_size`
    /// instead of copying it onto the `Outcome`.
    #[test]
    fn test_commit_artifact_size_overflow() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits {
            max_artifact_size: 4,
            ..VMLimits::default()
        };
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        let root_hash = [1u8; DIGEST_SIZE];
        let artifact = vec![1, 2, 3, 4, 5]; // 5 bytes > 4-byte limit
        let root_hash_ptr = 200u64;
        let artifact_ptr = 300u64;
        host.borrow_memory()
            .write(root_hash_ptr, &root_hash)
            .unwrap();
        host.borrow_memory().write(artifact_ptr, &artifact).unwrap();

        let root_hash_buf_ptr = 16u64;
        let artifact_buf_ptr = 32u64;
        prepare_guest_buf_descriptor(
            &host,
            root_hash_buf_ptr,
            root_hash_ptr,
            root_hash.len() as u64,
        );
        prepare_guest_buf_descriptor(&host, artifact_buf_ptr, artifact_ptr, artifact.len() as u64);

        let err = host
            .commit(root_hash_buf_ptr, artifact_buf_ptr)
            .unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::ArtifactSizeOverflow { size: 5, max: 4 })
        ));

        // The over-large artifact must not have been copied onto the state, and
        // the commit must not be marked as completed.
        assert!(host.borrow_logic().artifact.is_empty());
        assert_eq!(host.borrow_logic().root_hash, None);
        assert!(!host.borrow_logic().commit_called);
    }

    /// A return value larger than `max_return_value_size` traps before being
    /// copied onto the `Outcome`.
    #[test]
    fn test_value_return_size_cap() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits {
            max_return_value_size: 4,
            ..Default::default()
        };
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Payload of 5 bytes, one over the cap.
        let data = b"hello";
        let data_ptr = 400u64;
        host.borrow_memory().write(data_ptr, data).unwrap();

        // `ValueReturn` is `{ discriminant: u64, payload: Buffer }`: write the
        // `Ok` discriminant (0) then the payload `Buffer` descriptor 8 bytes on.
        let vr_ptr = 100u64;
        host.borrow_memory()
            .write(vr_ptr, &0u64.to_le_bytes())
            .unwrap();
        prepare_guest_buf_descriptor(&host, vr_ptr + 8, data_ptr, data.len() as u64);

        let err = host.value_return(vr_ptr).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::ReturnValueSizeOverflow { size: 5, max: 4 })
        ));
        // Nothing captured onto the outcome.
        assert!(host.borrow_logic().returns.is_none());
    }

    /// A handler name longer than `max_method_name_length` traps rather than
    /// stashing an arbitrarily large string on the event.
    #[test]
    fn test_emit_with_handler_size_cap() {
        let mut storage = SimpleMockStorage::new();
        let limits = VMLimits::default();
        let (mut logic, mut store) = setup_vm!(&mut storage, &limits, vec![]);
        let mut host = logic.host_functions(store.as_store_mut());

        // Minimal valid `Event { kind: Buffer, data: Buffer }` (each 16 bytes).
        let kind = b"k";
        let kind_ptr = 400u64;
        host.borrow_memory().write(kind_ptr, kind).unwrap();
        let event_ptr = 100u64;
        prepare_guest_buf_descriptor(&host, event_ptr, kind_ptr, kind.len() as u64);
        prepare_guest_buf_descriptor(&host, event_ptr + 16, 500u64, 0);

        // Handler descriptor claiming a length past `max_method_name_length`.
        // The length check trips before the (unread) payload slice is touched.
        let handler_ptr = 200u64;
        let oversized = limits.max_method_name_length + 1;
        prepare_guest_buf_descriptor(&host, handler_ptr, 600u64, oversized);

        let err = host.emit_with_handler(event_ptr, handler_ptr).unwrap_err();
        assert!(matches!(
            err,
            VMLogicError::HostError(HostError::EventHandlerSizeOverflow { .. })
        ));
        // The event must not have been recorded.
        assert!(host.borrow_logic().events.is_empty());
    }
}
