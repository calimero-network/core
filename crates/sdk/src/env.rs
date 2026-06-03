//! Environment functions and WASM interface for Calimero applications.
//!
//! This module provides the core environment functions that allow Calimero applications
//! to interact with the runtime, including logging, state management, and event emission.
//!
//! # Key Features
//!
//! - **Logging**: Structured logging with different levels (debug, info, warn, error)
//! - **State Management**: Persistent storage with key-value operations
//! - **Event Emission**: Event emission with optional callback handlers
//! - **Panic Handling**: Custom panic handling with location information
//! - **WASM Interface**: Low-level WASM function calls for runtime interaction
//!
//! # Usage
//!
//! Most applications will use the higher-level SDK functions rather than calling
//! these environment functions directly. However, they provide the foundation
//! for all Calimero application functionality.
//!
//! ```rust,no_run
//! use calimero_sdk::env;
//!
//! #[derive(serde::Serialize)]
//! struct MyEvent {
//!     data: String,
//! }
//!
//! impl calimero_sdk::event::AppEvent for MyEvent {
//!     fn kind(&self) -> std::borrow::Cow<'_, str> {
//!         "MyEvent".into()
//!     }
//!     fn data(&self) -> std::borrow::Cow<'_, [u8]> {
//!         serde_json::to_vec(self).unwrap().into()
//!     }
//! }
//!
//! impl calimero_sdk::event::AppEventExt for MyEvent {}
//!
//! // Log a message
//! env::log("Hello, Calimero!");
//!
//! // Emit an event
//! let my_event = MyEvent { data: "hello".to_string() };
//! env::emit(&my_event);
//! ```

#[cfg(target_arch = "wasm32")]
use std::panic::set_hook;

#[cfg(target_arch = "wasm32")]
use calimero_sys::{
    self as sys, Buffer, BufferMut, Event, Location, PtrSizedInt, Ref, RegisterId, ValueReturn,
    XCall,
};

use crate::event::AppEvent;

/// HTTP `fetch` host wrapper — WASM-only (no in-process mock equivalent).
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod ext;

/// Native mock host backing the in-process test harness. Off-`wasm32` the
/// `calimero_sys` imports don't exist, so [`crate::env`] routes here instead.
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub mod host;

/// Mirrors a committed root `Entry` (read from `calimero_storage`'s native mock)
/// into the SDK host storage at [`calimero_prelude::root_storage_key`], so
/// [`crate::state::read_raw`] observes it during `TestHost` migration tests.
///
/// Test-harness plumbing: the generated `TestState` bridge calls this after each
/// commit (it has access to both crates; `calimero_sdk` itself cannot depend on
/// `calimero_storage`). Native-only.
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub fn __test_seed_root(root_entry: Vec<u8>) {
    host::seed_storage(&calimero_prelude::root_storage_key(), root_entry);
}

/// Host-backed `tracing` subscriber (cargo feature `tracing`). Routes
/// `tracing` macro output through [`log`] so it reaches the execution outcome.
#[cfg(feature = "tracing")]
mod subscriber;

/// Sets the maximum `tracing` level forwarded to the host (feature `tracing`).
#[cfg(feature = "tracing")]
pub use subscriber::set_log_level;
/// Re-exported so apps can set the level without depending on `tracing`
/// directly, e.g. `env::set_log_level(env::LevelFilter::DEBUG)`.
#[cfg(feature = "tracing")]
pub use tracing::level_filters::LevelFilter;

/// Register ID used for data operations in the WASM runtime.
#[cfg(target_arch = "wasm32")]
const DATA_REGISTER: RegisterId = RegisterId::new(PtrSizedInt::MAX.as_usize() - 1);

/// Reports that a host function has no native mock equivalent.
///
/// A handful of host functions (cross-context calls, networked blobs, HTTP
/// fetch, signature verification) have no meaningful in-process behaviour. They
/// stay callable so app code compiles for tests, but invoking one under
/// [`crate::testing::TestHost`] panics with a clear message rather than the
/// opaque "only available when compiled for wasm32" `calimero_sys` stub.
#[cfg(not(target_arch = "wasm32"))]
#[track_caller]
#[cold]
fn unsupported_native(name: &str) -> ! {
    panic!("`env::{name}` is not supported by the in-process test harness (TestHost)");
}

/// Panics the application with caller location information.
///
/// This function provides a structured panic that includes the location
/// where the panic occurred, making debugging easier.
#[track_caller]
#[inline]
pub fn panic() -> ! {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::panic(Ref::new(&Location::caller()))
    }
    #[cfg(not(target_arch = "wasm32"))]
    panic!("application panicked");
}

/// Panics the application with a custom message and caller location.
///
/// This function provides a structured panic with a custom error message
/// and the location where the panic occurred.
///
/// # Parameters
///
/// * `message` - The custom panic message to display
#[track_caller]
#[inline]
pub fn panic_str(message: &str) -> ! {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::panic_utf8(
            Ref::new(&Buffer::from(message)),
            Ref::new(&Location::caller()),
        )
    }
    #[cfg(not(target_arch = "wasm32"))]
    panic!("{message}");
}

#[cfg(target_arch = "wasm32")]
#[track_caller]
#[inline]
fn expected_register<T>() -> T {
    panic_str("Expected a register to be set, but it was not.");
}

#[cfg(target_arch = "wasm32")]
#[track_caller]
#[inline]
fn expected_boolean<T>(e: u32) -> T {
    panic_str(&format!("Expected 0|1. Got {e}"));
}

/// Sets up a custom panic hook for better error reporting.
///
/// This function configures a panic hook that provides detailed error information
/// including the panic message and location, making debugging easier in the WASM environment.
///
/// Off-`wasm32` (under the test harness) this is a no-op: Rust's default panic
/// behaviour already surfaces the message and location to the test runner.
pub fn setup_panic_hook() {
    #[cfg(target_arch = "wasm32")]
    set_hook(Box::new(|info| {
        #[expect(clippy::option_if_let_else, reason = "Clearer this way")]
        let message = match info.payload().downcast_ref::<&'static str>() {
            Some(message) => *message,
            None => info
                .payload()
                .downcast_ref::<String>()
                .map_or("<no message>", |message| &**message),
        };

        unsafe {
            sys::panic_utf8(
                Ref::new(&Buffer::from(message)),
                Ref::new(&Location::from(info.location())),
            )
        }
    }));
}

/// Installs the host-backed `tracing` subscriber so `tracing` macros
/// (`info!`/`debug!`/…) emitted by the app — and by any crate it imports — are
/// forwarded to the host log and surface in the execution outcome.
///
/// No-op unless the `tracing` cargo feature is enabled. Idempotent: the
/// generated WASM exports call this at method entry (alongside
/// [`setup_panic_hook`]), so apps get `tracing` output with no manual setup.
/// Tune verbosity with [`set_log_level`].
#[inline]
pub fn init_logging() {
    #[cfg(feature = "tracing")]
    subscriber::init();
}

/// Marks code as unreachable for optimization purposes.
///
/// This function is used to indicate that certain code paths should never be reached.
/// It provides platform-specific unreachable instructions for optimal performance.
#[track_caller]
pub fn unreachable() -> ! {
    #[cfg(target_arch = "wasm32")]
    core::arch::wasm32::unreachable();

    #[cfg(not(target_arch = "wasm32"))]
    unreachable!()
}

#[cfg(target_arch = "wasm32")]
#[inline]
#[must_use]
pub fn register_len(register_id: RegisterId) -> Option<usize> {
    let len = unsafe { sys::register_len(register_id) };

    if len == PtrSizedInt::MAX {
        return None;
    }

    Some(len.as_usize())
}

#[cfg(target_arch = "wasm32")]
#[inline]
pub fn read_register(register_id: RegisterId) -> Option<Vec<u8>> {
    let len = register_len(register_id)?;

    let mut buffer = Vec::with_capacity(len);

    let succeed: bool = unsafe {
        buffer.set_len(len);

        sys::read_register(register_id, Ref::new(&BufferMut::new(&mut buffer)))
            .try_into()
            .unwrap_or_else(expected_boolean)
    };

    if !succeed {
        panic_str("Buffer is too small.");
    }

    Some(buffer)
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn read_register_sized<const N: usize>(register_id: RegisterId) -> Option<[u8; N]> {
    let len = register_len(register_id)?;

    let mut buffer = [0; N];

    #[expect(
        clippy::needless_borrows_for_generic_args,
        reason = "we don't want to copy the buffer, but write to the same one that's returned"
    )]
    let succeed: bool = unsafe {
        sys::read_register(register_id, Ref::new(&BufferMut::new(&mut buffer)))
            .try_into()
            .unwrap_or_else(expected_boolean)
    };

    if !succeed {
        panic_str(&format!(
            "register content length ({len}) does not match buffer length ({N})"
        ));
    }

    Some(buffer)
}

#[must_use]
pub fn context_id() -> [u8; 32] {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::context_id(DATA_REGISTER) }
        read_register_sized(DATA_REGISTER).expect("Must have context identity.")
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::context_id()
}

#[must_use]
pub fn executor_id() -> [u8; 32] {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::executor_id(DATA_REGISTER) }
        read_register_sized(DATA_REGISTER).expect("Must have executor identity.")
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::executor_id()
}

#[inline]
#[must_use]
pub fn input() -> Option<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::input(DATA_REGISTER) }
        read_register(DATA_REGISTER)
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::input()
}

#[inline]
pub fn value_return<T, E>(result: &Result<T, E>)
where
    T: AsRef<[u8]>,
    E: AsRef<[u8]>,
{
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::value_return(Ref::new(&ValueReturn::from(result.as_ref())))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bytes: &[u8] = match result {
            Ok(value) => value.as_ref(),
            Err(error) => error.as_ref(),
        };
        host::value_return(bytes);
    }
}

/// Logs a message to the runtime's logging system.
///
/// This function sends a log message to the runtime, which will be displayed
/// in the application logs. This is the primary way to output debug information
/// from Calimero applications.
///
/// # Parameters
///
/// * `message` - The message to log
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// env::log("Application started");
/// let item_id = "item_123";
/// env::log(&format!("Processing item: {}", item_id));
/// ```
#[inline]
pub fn log(message: &str) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::log_utf8(Ref::new(&Buffer::from(message)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::log(message);
}

/// Emits an event through the runtime without any callback handler.
///
/// This function sends an event to the runtime for processing. The event
/// will be available to external systems but no callback handler will be executed.
///
/// # Parameters
///
/// * `event` - The event to emit (must implement `AppEvent`)
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// #[derive(serde::Serialize)]
/// struct MyEvent {
///     data: String,
/// }
///
/// impl calimero_sdk::event::AppEvent for MyEvent {
///     fn kind(&self) -> std::borrow::Cow<'_, str> {
///         "MyEvent".into()
///     }
///     fn data(&self) -> std::borrow::Cow<'_, [u8]> {
///         serde_json::to_vec(self).unwrap().into()
///     }
/// }
///
/// impl calimero_sdk::event::AppEventExt for MyEvent {}
///
/// let my_event = MyEvent { data: "hello".to_string() };
/// env::emit(&my_event);
/// ```
#[inline]
pub fn emit<T: AppEvent>(event: &T) {
    let kind = event.kind();
    let data = event.data();

    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::emit(Ref::new(&Event::new(&kind, &data)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::emit(&kind, &data);
}

/// Emits an event through the runtime with a callback handler.
///
/// This function sends an event to the runtime and arranges for the specified
/// handler method to be called after the event is processed. The handler
/// method should be defined in your application.
///
/// # Parameters
///
/// * `event` - The event to emit (must implement `AppEvent`)
/// * `handler` - The name of the handler method to call
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// #[derive(serde::Serialize)]
/// struct MyEvent {
///     data: String,
/// }
///
/// impl calimero_sdk::event::AppEvent for MyEvent {
///     fn kind(&self) -> std::borrow::Cow<'_, str> {
///         "MyEvent".into()
///     }
///     fn data(&self) -> std::borrow::Cow<'_, [u8]> {
///         serde_json::to_vec(self).unwrap().into()
///     }
/// }
///
/// impl calimero_sdk::event::AppEventExt for MyEvent {}
///
/// let my_event = MyEvent { data: "hello".to_string() };
/// env::emit_with_handler(&my_event, "my_handler");
/// ```
#[inline]
pub fn emit_with_handler<T: AppEvent>(event: &T, handler: &str) {
    let kind = event.kind();
    let data = event.data();

    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::emit_with_handler(
            Ref::new(&Event::new(&kind, &data)),
            Ref::new(&Buffer::from(handler.as_bytes())),
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::emit_with_handler(&kind, &data, handler);
}

/// Makes a cross-context call to be executed after the current execution completes.
///
/// This function queues a call to another context that will be executed locally
/// after the current execution finishes. The call will be executed on the specified
/// context with the given function name and parameters.
///
/// # Parameters
///
/// * `context_id` - The 32-byte ID of the context to call
/// * `function` - The name of the function to call in the target context
/// * `params` - The parameters to pass to the function (typically JSON-encoded)
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// let target_context = [0u8; 32]; // The context ID to call
/// let params = serde_json::to_vec(&"hello").unwrap();
/// env::xcall(&target_context, "my_function", &params);
/// ```
#[inline]
pub fn xcall(context_id: &[u8; 32], function: &str, params: &[u8]) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::xcall(Ref::new(&XCall::new(context_id, function, params)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (context_id, function, params);
        unsupported_native("xcall");
    }
}

/// Commits state changes to the runtime.
///
/// This function commits the current state changes along with a root hash
/// and artifact to the runtime for persistence.
///
/// # Parameters
///
/// * `root_hash` - The root hash of the state tree
/// * `artifact` - The artifact data to commit
pub fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::commit(
            Ref::new(&Buffer::from(&root_hash[..])),
            Ref::new(&Buffer::from(artifact)),
        );
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::commit(root_hash, artifact);
}

/// Reads a value from persistent storage.
///
/// This function retrieves a value from the application's persistent storage
/// using the provided key. Returns `None` if the key doesn't exist.
///
/// # Parameters
///
/// * `key` - The storage key to read from
///
/// # Returns
///
/// * `Some(Vec<u8>)` - The stored value if the key exists
/// * `None` - If the key doesn't exist
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if let Some(value) = env::storage_read(b"my_key") {
///     println!("Found value: {:?}", value);
/// }
/// ```
#[inline]
pub fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::storage_read(Ref::new(&Buffer::from(key)), DATA_REGISTER) }
            .try_into()
            .unwrap_or_else(expected_boolean::<bool>)
            .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::storage_read(key)
}

/// Removes a value from persistent storage.
///
/// This function removes a key-value pair from the application's persistent storage.
///
/// # Parameters
///
/// * `key` - The storage key to remove
///
/// # Returns
///
/// * `true` - If the key existed and was removed
/// * `false` - If the key didn't exist
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if env::storage_remove(b"my_key") {
///     println!("Key was removed");
/// } else {
///     println!("Key didn't exist");
/// }
/// ```
#[inline]
pub fn storage_remove(key: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::storage_remove(Ref::new(&Buffer::from(key)), DATA_REGISTER).try_into() }
            .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::storage_remove(key)
}

/// Writes a value to persistent storage.
///
/// This function stores a key-value pair in the application's persistent storage.
/// The value will be available across application restarts.
///
/// # Parameters
///
/// * `key` - The storage key to write to
/// * `value` - The value to store
///
/// # Returns
///
/// * `true` - A previous value existed under `key` and was evicted.
/// * `false` - No previous value; a new entry was inserted.
///
/// The return value is **not** a success/failure indicator. The write
/// itself either succeeds or traps the WASM module with a `HostError`
/// (`KeyLengthOverflow`, `ValueLengthOverflow`, `InvalidMemoryAccess`),
/// which propagates as a runtime error, not as `false`. Callers that
/// don't care whether the key existed before can safely discard the bool.
///
/// **Note:** [`private_storage_write`] uses a different return-value
/// convention (true = succeeded, false = private storage unavailable)
/// because that backend can be absent at runtime, unlike main storage
/// which is always available.
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if env::storage_write(b"my_key", b"my_value") {
///     println!("Overwrote an existing value");
/// } else {
///     println!("Inserted a new entry");
/// }
/// ```
#[inline]
pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            sys::storage_write(
                Ref::new(&Buffer::from(key)),
                Ref::new(&Buffer::from(value)),
                DATA_REGISTER,
            )
            .try_into()
        }
        .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::storage_write(key, value)
}

// ==================== Ordered Secondary Index (SortedMap) ====================
//
// Node-local, NOT synchronized. Keys are the unhashed `collection ‖ order_key`
// so the backend keeps them in byte (= logical key) order. Only `SortedMap`
// (via the `MainStorage` adaptor) calls these.

/// Insert/overwrite `key -> value` in the ordered index. Returns whether the
/// host persisted the write (so the caller can avoid stamping a stale
/// index-validity marker on failure).
#[inline]
pub fn storage_index_set(key: &[u8], value: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            sys::storage_index_set(Ref::new(&Buffer::from(key)), Ref::new(&Buffer::from(value)))
        }
        .try_into()
        .unwrap_or_else(expected_boolean)
    }
    // `SortedMap`'s ordered index is served by `calimero_storage`'s own native
    // mock (a process-local `BTreeMap`) under the test harness, so this SDK-level
    // host hook is never reached off-wasm.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (key, value);
        unsupported_native("storage_index_set")
    }
}

/// Remove `key` from the ordered index. Returns whether the host persisted the
/// write (see [`storage_index_set`]).
#[inline]
pub fn storage_index_remove(key: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::storage_index_remove(Ref::new(&Buffer::from(key))) }
            .try_into()
            .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        unsupported_native("storage_index_remove")
    }
}

/// Remove every ordered-index key beginning with `prefix`. Returns whether the
/// host persisted the write (see [`storage_index_set`]).
#[inline]
pub fn storage_index_remove_prefix(prefix: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::storage_index_remove_prefix(Ref::new(&Buffer::from(prefix))) }
            .try_into()
            .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = prefix;
        unsupported_native("storage_index_remove_prefix")
    }
}

/// Scan the ordered index over `[lo, hi)`, ascending, after `offset` and
/// capped at `limit` (`None` = unbounded). Decodes the host's length-prefixed
/// reply (`count:u32`, then per pair `klen:u32, k, vlen:u32, v`, little-endian).
#[inline]
pub fn storage_index_scan(
    lo: &[u8],
    hi: &[u8],
    offset: usize,
    limit: Option<usize>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    #[cfg(target_arch = "wasm32")]
    {
        // Encode the limit as `n + 1`, with `0` = unbounded. A `MAX` sentinel
        // would be ambiguous: `usize` is 32-bit on wasm32, so `usize::MAX`
        // (`u32::MAX`) would not equal the host's `u64::MAX`. `0` is unambiguous
        // on any width.
        let limit_raw = limit.map_or(0, |n| n.saturating_add(1));
        let found: bool = unsafe {
            sys::storage_index_scan(
                Ref::new(&Buffer::from(lo)),
                Ref::new(&Buffer::from(hi)),
                PtrSizedInt::new(offset),
                PtrSizedInt::new(limit_raw),
                DATA_REGISTER,
            )
            .try_into()
        }
        .unwrap_or_else(expected_boolean);

        if !found {
            return Vec::new();
        }
        let buf = read_register(DATA_REGISTER).unwrap_or_else(expected_register);
        decode_index_pairs(&buf)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (lo, hi, offset, limit);
        unsupported_native("storage_index_scan")
    }
}

/// The largest `(key, value)` in the ordered index over `[lo, hi)` — a reverse
/// seek backing `SortedMap::last`.
#[inline]
pub fn storage_index_last(lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    #[cfg(target_arch = "wasm32")]
    {
        let found: bool = unsafe {
            sys::storage_index_last(
                Ref::new(&Buffer::from(lo)),
                Ref::new(&Buffer::from(hi)),
                DATA_REGISTER,
            )
            .try_into()
        }
        .unwrap_or_else(expected_boolean);

        if !found {
            return None;
        }
        let buf = read_register(DATA_REGISTER).unwrap_or_else(expected_register);
        decode_index_pairs(&buf).into_iter().next()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (lo, hi);
        unsupported_native("storage_index_last")
    }
}

/// Decode the length-prefixed scan reply produced by the host's
/// `encode_index_pairs`. Malformed/truncated input yields what was parsed so
/// far (the host controls this buffer, so it's well-formed in practice).
///
/// WASM-only: the ordered-index host hooks are unreachable off-wasm (see
/// `storage_index_set`), so the decoder has no native caller.
#[cfg(target_arch = "wasm32")]
fn decode_index_pairs(buf: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    let take_u32 = |buf: &[u8], pos: &mut usize| -> Option<usize> {
        let end = pos.checked_add(4)?;
        let n = u32::from_le_bytes(buf.get(*pos..end)?.try_into().ok()?) as usize;
        *pos = end;
        Some(n)
    };
    let Some(count) = take_u32(buf, &mut pos) else {
        return out;
    };
    for _ in 0..count {
        let Some(klen) = take_u32(buf, &mut pos) else {
            break;
        };
        let Some(key) = buf.get(pos..pos + klen).map(<[u8]>::to_vec) else {
            break;
        };
        pos += klen;
        let Some(vlen) = take_u32(buf, &mut pos) else {
            break;
        };
        let Some(value) = buf.get(pos..pos + vlen).map(<[u8]>::to_vec) else {
            break;
        };
        pos += vlen;
        out.push((key, value));
    }
    out
}

// ==================== Private Storage Functions ====================
// These functions operate on node-local storage that is NOT synchronized across nodes.

/// Reads a value from private (node-local) storage.
///
/// Private storage is NOT synchronized across nodes - it remains local to this node only.
/// This is useful for storing secrets, node-specific configuration, or other data that
/// should not be shared with other nodes in the context.
///
/// # Parameters
///
/// * `key` - The storage key to read
///
/// # Returns
///
/// * `Some(Vec<u8>)` - The value if found
/// * `None` - If the key doesn't exist or private storage is not available
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if let Some(value) = env::private_storage_read(b"my_secret") {
///     println!("Found private value: {:?}", value);
/// }
/// ```
#[inline]
pub fn private_storage_read(key: &[u8]) -> Option<Vec<u8>> {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::private_storage_read(Ref::new(&Buffer::from(key)), DATA_REGISTER) }
            .try_into()
            .unwrap_or_else(expected_boolean::<bool>)
            .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::private_storage_read(key)
}

/// Removes a value from private (node-local) storage.
///
/// Private storage is NOT synchronized across nodes - it remains local to this node only.
///
/// # Parameters
///
/// * `key` - The storage key to remove
///
/// # Returns
///
/// * `true` - If the key existed and was removed
/// * `false` - If the key didn't exist or private storage is not available
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if env::private_storage_remove(b"my_secret") {
///     println!("Private key was removed");
/// }
/// ```
#[inline]
pub fn private_storage_remove(key: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            sys::private_storage_remove(Ref::new(&Buffer::from(key)), DATA_REGISTER).try_into()
        }
        .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::private_storage_remove(key)
}

/// Writes a value to private (node-local) storage.
///
/// Private storage is NOT synchronized across nodes - it remains local to this node only.
/// This is useful for storing secrets, node-specific configuration, or other data that
/// should not be shared with other nodes in the context.
///
/// # Parameters
///
/// * `key` - The storage key to write to
/// * `value` - The value to store
///
/// # Returns
///
/// * `true` - The write succeeded (a new entry was inserted, or an
///   existing entry was overwritten).
/// * `false` - Private storage is not available on this node (the
///   backend is optional and may be absent at runtime; the write
///   was a no-op).
///
/// Other failure modes (`KeyLengthOverflow`, `ValueLengthOverflow`,
/// `InvalidMemoryAccess`) trap the WASM module via `HostError` rather
/// than returning `false`.
///
/// **Note:** [`storage_write`] uses a different return-value
/// convention (true = previous value evicted, false = new key
/// inserted) because main storage is always available, so a
/// success/failure bool would be tautological.
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if env::private_storage_write(b"my_secret", b"secret_value") {
///     println!("Private value stored successfully");
/// } else {
///     println!("Private storage is not available on this node");
/// }
/// ```
#[inline]
pub fn private_storage_write(key: &[u8], value: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            sys::private_storage_write(Ref::new(&Buffer::from(key)), Ref::new(&Buffer::from(value)))
                .try_into()
        }
        .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::private_storage_write(key, value)
}

/// Fill the buffer with random bytes.
///
/// Under the in-process test harness (off-`wasm32`) this is backed by a
/// deterministic, **non-cryptographic** PRNG so test runs are reproducible — do
/// not rely on it for security properties in tests.
#[inline]
pub fn random_bytes(buf: &mut [u8]) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        sys::random_bytes(Ref::new(&BufferMut::new(buf)))
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::random_bytes(buf);
}

/// Gets the current time.
#[inline]
#[must_use]
pub fn time_now() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        let mut bytes = [0; 8];

        #[expect(
            clippy::needless_borrows_for_generic_args,
            reason = "we don't want to copy the buffer, but write to the same one that's returned"
        )]
        unsafe {
            sys::time_now(Ref::new(&BufferMut::new(&mut bytes)));
        }

        u64::from_le_bytes(bytes)
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::time_now()
}

/// Verifies an Ed25519 signature.
///
/// This function calls the host environment to cryptographically verify that
/// a given signature was produced by the given public key for the given message.
///
/// # Arguments
///
/// * `signature` - The 64-byte Ed25519 signature.
/// * `public_key` - The 32-byte Ed25519 public key.
/// * `message` - The message bytes that were signed.
///
/// # Returns
///
/// * `true` if the signature is valid.
/// * `false` if the signature is invalid.
///
/// # Panics
///
/// Panics if the host returns a value other than `0` or `1`.
#[inline]
pub fn ed25519_verify(signature: &[u8; 64], public_key: &[u8; 32], message: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        // Create buffer descriptors for the host
        let signature_buf = Buffer::from(&signature[..]);
        let public_key_buf = Buffer::from(&public_key[..]);
        let message_buf = Buffer::from(message);

        // Call the host function via FFI
        let result = unsafe {
            sys::ed25519_verify(
                Ref::new(&signature_buf),
                Ref::new(&public_key_buf),
                Ref::new(&message_buf),
            )
        };

        // Convert the sys::Bool (repr(C) u32) to a bool, panicking if it's not 0 or 1.
        result.try_into().unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (signature, public_key, message);
        // Pulling a crypto backend into the SDK just for the test mock isn't
        // worth it; tests that need real verification can call the runtime.
        unsupported_native("ed25519_verify");
    }
}

// ========================================
// STREAMING BLOB API
// ========================================

/// Create a new blob write handle for streaming data.
/// Returns a file descriptor that can be used with blob_write() and blob_close().
pub fn blob_create() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::blob_create() }.as_usize() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::blob_create()
}

/// Open a blob for reading by its 32-byte ID.
/// Returns a file descriptor that can be used with blob_read() and blob_close().
/// Returns 0 if the blob is not found.
pub fn blob_open(blob_id: &[u8; 32]) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::blob_open(Ref::new(&Buffer::from(&blob_id[..]))) }.as_usize() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::blob_open(blob_id)
}

/// Read data from a blob handle opened with blob_open().
/// Reads into the provided buffer and returns the number of bytes read.
/// Returns 0 when end of blob is reached.
pub fn blob_read(fd: u64, buffer: &mut [u8]) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            sys::blob_read(
                PtrSizedInt::new(fd as usize),
                Ref::new(&BufferMut::new(buffer)),
            )
        }
        .as_usize() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::blob_read(fd, buffer)
}

/// Write data to a blob handle created with blob_create().
/// Returns the number of bytes written.
pub fn blob_write(fd: u64, data: &[u8]) -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { sys::blob_write(PtrSizedInt::new(fd as usize), Ref::new(&Buffer::from(data))) }
            .as_usize() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::blob_write(fd, data)
}

/// Close a blob handle and finalize the blob.
///
/// For write handles: Finalizes the blob and returns its 32-byte ID.
/// For read handles: Returns the original blob's ID and cleans up the handle.
/// Panics if the operation fails (e.g. blob finalization fails for write handles).
pub fn blob_close(fd: u64) -> [u8; 32] {
    #[cfg(target_arch = "wasm32")]
    {
        let mut blob_id_buf = [0_u8; 32];
        let success: bool = unsafe {
            sys::blob_close(
                PtrSizedInt::new(fd as usize),
                Ref::new(&BufferMut::new(&mut blob_id_buf)),
            )
            .try_into()
        }
        .unwrap_or_else(expected_boolean);

        if success {
            blob_id_buf
        } else {
            panic_str("Blob operation failed")
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    host::blob_close(fd).unwrap_or_else(|| panic_str("Blob operation failed"))
}

// ========================================
// NETWORK-AWARE BLOB API
// ========================================

/// Announce a blob to a specific context for network discovery.
/// This makes the blob discoverable by other nodes in the context.
/// Returns true if the announcement was successful.
///
/// # Security
/// For security reasons, a context can only announce blobs to itself.
/// If the provided context_id doesn't match the current context, this function returns false.
///
/// # Arguments
/// * `blob_id` - The 32-byte ID of the blob to announce
/// * `target_context_id` - The 32-byte ID of the context to announce the blob in (must match current context)
///
/// # Example
/// ```no_run
/// use calimero_sdk::env;
///
/// // Create and write a blob
/// let fd = env::blob_create();
/// env::blob_write(fd, b"Hello, World!");
/// let blob_id = env::blob_close(fd);
///
/// // Announce it to the current context
/// let current_context = env::context_id();
/// let announced = env::blob_announce_to_context(&blob_id, &current_context);
/// ```
pub fn blob_announce_to_context(blob_id: &[u8; 32], target_context_id: &[u8; 32]) -> bool {
    // Security check: only allow announcing to the current context
    let current_context = context_id();
    if current_context != *target_context_id {
        // Attempting to announce to a different context is not allowed
        return false;
    }

    #[cfg(target_arch = "wasm32")]
    {
        unsafe {
            sys::blob_announce_to_context(
                Ref::new(&Buffer::from(&blob_id[..])),
                Ref::new(&Buffer::from(&target_context_id[..])),
            )
            .try_into()
        }
        .unwrap_or_else(expected_boolean)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // The in-process host has no network; the announce is a no-op that
        // succeeds once the (already-checked) context match holds.
        let _ = blob_id;
        true
    }
}
