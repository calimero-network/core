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

use std::panic::set_hook;

use calimero_sys::{
    self as sys, Buffer, BufferMut, Event, Location, PtrSizedInt, Ref, RegisterId, ValueReturn,
    XCall,
};

use crate::event::AppEvent;

#[doc(hidden)]
pub mod ext;

/// Register ID used for data operations in the WASM runtime.
const DATA_REGISTER: RegisterId = RegisterId::new(PtrSizedInt::MAX.as_usize() - 1);

/// Panics the application with caller location information.
///
/// This function provides a structured panic that includes the location
/// where the panic occurred, making debugging easier.
#[track_caller]
#[inline]
pub fn panic() -> ! {
    unsafe { sys::panic(Ref::new(&Location::caller())) }
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
    unsafe {
        sys::panic_utf8(
            Ref::new(&Buffer::from(message)),
            Ref::new(&Location::caller()),
        )
    }
}

#[track_caller]
#[inline]
fn expected_register<T>() -> T {
    panic_str("Expected a register to be set, but it was not.");
}

#[track_caller]
#[inline]
fn expected_boolean<T>(e: u32) -> T {
    panic_str(&format!("Expected 0|1. Got {e}"));
}

/// Sets up a custom panic hook for better error reporting.
///
/// This function configures a panic hook that provides detailed error information
/// including the panic message and location, making debugging easier in the WASM environment.
pub fn setup_panic_hook() {
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

#[inline]
#[must_use]
pub fn register_len(register_id: RegisterId) -> Option<usize> {
    let len = unsafe { sys::register_len(register_id) };

    if len == PtrSizedInt::MAX {
        return None;
    }

    Some(len.as_usize())
}

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
    unsafe { sys::context_id(DATA_REGISTER) }
    read_register_sized(DATA_REGISTER).expect("Must have context identity.")
}

#[must_use]
pub fn executor_id() -> [u8; 32] {
    unsafe { sys::executor_id(DATA_REGISTER) }
    read_register_sized(DATA_REGISTER).expect("Must have executor identity.")
}

#[inline]
#[must_use]
pub fn input() -> Option<Vec<u8>> {
    unsafe { sys::input(DATA_REGISTER) }
    read_register(DATA_REGISTER)
}

#[inline]
pub fn value_return<T, E>(result: &Result<T, E>)
where
    T: AsRef<[u8]>,
    E: AsRef<[u8]>,
{
    unsafe { sys::value_return(Ref::new(&ValueReturn::from(result.as_ref()))) }
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
    unsafe { sys::log_utf8(Ref::new(&Buffer::from(message))) }
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

    unsafe { sys::emit(Ref::new(&Event::new(&kind, &data))) }
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

    unsafe {
        sys::emit_with_handler(
            Ref::new(&Event::new(&kind, &data)),
            Ref::new(&Buffer::from(handler.as_bytes())),
        );
    }
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
    unsafe { sys::xcall(Ref::new(&XCall::new(context_id, function, params))) }
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
    unsafe {
        sys::commit(
            Ref::new(&Buffer::from(&root_hash[..])),
            Ref::new(&Buffer::from(artifact)),
        );
    }
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
    unsafe { sys::storage_read(Ref::new(&Buffer::from(key)), DATA_REGISTER) }
        .try_into()
        .unwrap_or_else(expected_boolean::<bool>)
        .then(|| read_register(DATA_REGISTER).unwrap_or_else(expected_register))
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
    unsafe { sys::storage_remove(Ref::new(&Buffer::from(key)), DATA_REGISTER).try_into() }
        .unwrap_or_else(expected_boolean)
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
/// * `true` - If the write operation succeeded
/// * `false` - If the write operation failed
///
/// # Example
///
/// ```rust,no_run
/// use calimero_sdk::env;
///
/// if env::storage_write(b"my_key", b"my_value") {
///     println!("Value stored successfully");
/// }
/// ```
#[inline]
pub fn storage_write(key: &[u8], value: &[u8]) -> bool {
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

/// Fill the buffer with random bytes.
#[inline]
pub fn random_bytes(buf: &mut [u8]) {
    unsafe { sys::random_bytes(Ref::new(&BufferMut::new(buf))) }
}

/// Gets the current time.
#[inline]
#[must_use]
pub fn time_now() -> u64 {
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

// ========================================
// STREAMING BLOB API
// ========================================

/// Create a new blob write handle for streaming data.
/// Returns a file descriptor that can be used with blob_write() and blob_close().
pub fn blob_create() -> u64 {
    unsafe { sys::blob_create() }.as_usize() as u64
}

/// Open a blob for reading by its 32-byte ID.
/// Returns a file descriptor that can be used with blob_read() and blob_close().
/// Returns 0 if the blob is not found.
pub fn blob_open(blob_id: &[u8; 32]) -> u64 {
    unsafe { sys::blob_open(Ref::new(&Buffer::from(&blob_id[..]))) }.as_usize() as u64
}

/// Read data from a blob handle opened with blob_open().
/// Reads into the provided buffer and returns the number of bytes read.
/// Returns 0 when end of blob is reached.
pub fn blob_read(fd: u64, buffer: &mut [u8]) -> u64 {
    unsafe {
        sys::blob_read(
            PtrSizedInt::new(fd as usize),
            Ref::new(&BufferMut::new(buffer)),
        )
    }
    .as_usize() as u64
}

/// Write data to a blob handle created with blob_create().
/// Returns the number of bytes written.
pub fn blob_write(fd: u64, data: &[u8]) -> u64 {
    unsafe { sys::blob_write(PtrSizedInt::new(fd as usize), Ref::new(&Buffer::from(data))) }
        .as_usize() as u64
}

/// Close a blob handle and finalize the blob.
///
/// For write handles: Finalizes the blob and returns its 32-byte ID.
/// For read handles: Returns the original blob's ID and cleans up the handle.
/// Panics if the operation fails (e.g. blob finalization fails for write handles).
pub fn blob_close(fd: u64) -> [u8; 32] {
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

    unsafe {
        sys::blob_announce_to_context(
            Ref::new(&Buffer::from(&blob_id[..])),
            Ref::new(&Buffer::from(&target_context_id[..])),
        )
        .try_into()
    }
    .unwrap_or_else(expected_boolean)
}
