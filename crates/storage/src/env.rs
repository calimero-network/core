//! Environment bindings for the storage crate.

#[cfg(target_arch = "wasm32")]
use calimero_vm as imp;
#[cfg(not(target_arch = "wasm32"))]
use mocked as imp;

use std::cell::Cell;

use crate::logical_clock::HybridTimestamp;
use crate::store::Key;

// ============================================================================
// Merge Mode Flag
// ============================================================================
//
// During CRDT merge operations, we must NOT generate new timestamps via time_now().
// If we generate local timestamps during merge, different nodes get different values,
// causing hash divergence even when the logical state is identical.
//
// This flag is set during merge_root_state() to prevent timestamp generation.
// When in merge mode:
// - Element::update() skips setting updated_at = time_now()
// - CollectionMut::drop() skips timestamp updates
// - This ensures merge is deterministic across nodes

thread_local! {
    static MERGE_MODE: Cell<bool> = const { Cell::new(false) };
}

/// Check if we're currently in merge mode (timestamp generation disabled).
#[must_use]
pub fn in_merge_mode() -> bool {
    MERGE_MODE.with(|m| m.get())
}

/// Execute a closure with merge mode enabled.
///
/// During merge mode, timestamp generation is disabled to ensure
/// deterministic results across nodes.
pub fn with_merge_mode<R>(f: impl FnOnce() -> R) -> R {
    MERGE_MODE.with(|m| m.set(true));
    let result = f();
    MERGE_MODE.with(|m| m.set(false));
    result
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
/// Runtime-provided storage environment used by host functions.
///
/// The JS runtime passes a `RuntimeEnv` down when it wants the storage crate to
/// talk to the live `RuntimeStorage` inside `VMLogic` instead of the default
/// mock/WASM adapters.  The environment packages read/write/remove callbacks
/// that close over the current storage trait object.  While the host function is
/// executing we install this environment thread-locally so every
/// `Interface::<MainStorage>::*` call can reach the real context storage.
pub struct RuntimeEnv {
    storage_read: std::rc::Rc<dyn Fn(&Key) -> Option<Vec<u8>>>,
    storage_write: std::rc::Rc<dyn Fn(Key, &[u8]) -> bool>,
    storage_remove: std::rc::Rc<dyn Fn(&Key) -> bool>,
    context_id: [u8; 32],
    executor_id: [u8; 32],
}

#[cfg(not(target_arch = "wasm32"))]
impl RuntimeEnv {
    #[must_use]
    /// Creates a new runtime environment with host-provided storage callbacks.
    ///
    /// The callbacks are reference-counted closures so they stay valid for the
    /// duration of the host call but can still hand mutable access to the
    /// underlying storage when invoked from the storage crate.
    pub fn new(
        storage_read: std::rc::Rc<dyn Fn(&Key) -> Option<Vec<u8>>>,
        storage_write: std::rc::Rc<dyn Fn(Key, &[u8]) -> bool>,
        storage_remove: std::rc::Rc<dyn Fn(&Key) -> bool>,
        context_id: [u8; 32],
        executor_id: [u8; 32],
    ) -> Self {
        Self {
            storage_read,
            storage_write,
            storage_remove,
            context_id,
            executor_id,
        }
    }

    #[must_use]
    /// Returns the storage read callback.
    pub fn storage_read(&self) -> std::rc::Rc<dyn Fn(&Key) -> Option<Vec<u8>>> {
        self.storage_read.clone()
    }

    #[must_use]
    /// Returns the storage write callback.
    pub fn storage_write(&self) -> std::rc::Rc<dyn Fn(Key, &[u8]) -> bool> {
        self.storage_write.clone()
    }

    #[must_use]
    /// Returns the storage remove callback.
    pub fn storage_remove(&self) -> std::rc::Rc<dyn Fn(&Key) -> bool> {
        self.storage_remove.clone()
    }

    #[must_use]
    /// Returns the current context identifier.
    pub const fn context_id(&self) -> [u8; 32] {
        self.context_id
    }

    #[must_use]
    /// Returns the current executor identifier.
    pub const fn executor_id(&self) -> [u8; 32] {
        self.executor_id
    }
}

#[cfg(not(target_arch = "wasm32"))]
/// Executes `f` with the provided runtime environment installed.
pub fn with_runtime_env<R>(env: RuntimeEnv, f: impl FnOnce() -> R) -> R {
    mocked::with_runtime_env(env, f)
}

/// Commits the root hash to the runtime.
///
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
    imp::commit(root_hash, artifact);
}

/// Reads data from persistent storage.
///
/// # Parameters
///
/// * `key` - The key to read data from.
///
#[must_use]
pub fn storage_read(key: Key) -> Option<Vec<u8>> {
    imp::storage_read(key)
}

/// Removes data from persistent storage.
///
/// # Parameters
///
/// * `key` - The key to remove.
///
#[must_use]
pub fn storage_remove(key: Key) -> bool {
    imp::storage_remove(key)
}

/// Writes data to persistent storage.
///
/// # Parameters
///
/// * `key`   - The key to write data to.
/// * `value` - The data to write.
///
#[must_use]
pub fn storage_write(key: Key, value: &[u8]) -> bool {
    imp::storage_write(key, value)
}

/// Fill the buffer with random bytes.
///
/// # Parameters
///
/// * `buf` - The buffer to fill with random bytes.
///
pub fn random_bytes(buf: &mut [u8]) {
    imp::random_bytes(buf);
}

/// Get the current time.
#[must_use]
pub fn time_now() -> u64 {
    imp::time_now()
}

/// Verifies an Ed25519 signature.
///
/// On WASM, this calls the host environment.
/// In tests, this uses a pure-Rust implementation.
#[must_use]
pub fn ed25519_verify(signature: &[u8; 64], public_key: &[u8; 32], message: &[u8]) -> bool {
    imp::ed25519_verify(signature, public_key, message)
}

/// Returns the current context ID.
///
/// In WASM, this calls the host function. In tests, returns a fixed value.
#[must_use]
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn context_id() -> [u8; 32] {
    imp::context_id()
}

/// Returns the current executor ID (the public key of the transaction signer).
///
/// In WASM, this calls the host function. In tests, returns a fixed value.
#[must_use]
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn executor_id() -> [u8; 32] {
    imp::executor_id()
}

/// Prints the log.
///
/// In WASM, this calls `calimero_sdk::env::log()`, which calls the host function.
/// In tests, it uses plain `println!()`.
#[expect(clippy::missing_const_for_fn, reason = "Cannot be const here")]
pub fn log(message: &str) {
    imp::log(message);
}

/// Get hybrid timestamp (auto-increments logical clock).
#[must_use]
pub fn hlc_timestamp() -> HybridTimestamp {
    imp::hlc_timestamp()
}

/// Update HLC with remote timestamp (preserves causality).
///
/// When syncing deltas from remote nodes, call this with each delta's HLC timestamp
/// to ensure the local clock observes remote operations and maintains causal ordering.
///
/// # Errors
///
/// Returns `Err(())` if the remote timestamp is >5s in the future (drift protection).
pub fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ()> {
    imp::update_hlc(remote_ts)
}

/// Reset for testing.
#[cfg(test)]
pub fn reset_for_testing() {
    imp::reset_for_testing();
}

/// Set executor ID for testing purposes
#[cfg(test)]
pub fn set_executor_id(id: [u8; 32]) {
    imp::set_executor_id(id);
}

#[cfg(target_arch = "wasm32")]
mod calimero_vm {
    use std::cell::RefCell;

    use calimero_sdk::env;

    use crate::logical_clock::{HybridTimestamp, LogicalClock};
    use crate::store::Key;

    thread_local! {
        static WASM_HLC: RefCell<Option<LogicalClock>> = const { RefCell::new(None) };
    }

    fn ensure_hlc_initialized() {
        WASM_HLC.with(|hlc_cell| {
            if hlc_cell.borrow().is_none() {
                // Use executor ID (node identity) as deterministic seed for HLC ID
                // This ensures each node has a unique but deterministic HLC ID
                let executor_id = env::executor_id();
                *hlc_cell.borrow_mut() = Some(LogicalClock::new(|buf| {
                    // Use executor ID to deterministically generate HLC ID
                    for (i, byte) in executor_id.iter().enumerate().take(buf.len()) {
                        buf[i] = *byte;
                    }
                    // Fill remaining bytes if buf is longer than executor_id
                    for i in executor_id.len()..buf.len() {
                        buf[i] = executor_id[i % executor_id.len()];
                    }
                }));
            }
        });
    }

    /// Commits the root hash to the runtime.
    pub(super) fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
        env::commit(root_hash, artifact);
    }

    /// Reads data from persistent storage.
    pub(super) fn storage_read(key: Key) -> Option<Vec<u8>> {
        env::storage_read(&key.to_bytes())
    }

    /// Removes data from persistent storage.
    pub(super) fn storage_remove(key: Key) -> bool {
        env::storage_remove(&key.to_bytes())
    }

    /// Writes data to persistent storage.
    pub(super) fn storage_write(key: Key, value: &[u8]) -> bool {
        env::storage_write(&key.to_bytes(), value)
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        env::random_bytes(buf)
    }

    /// Return the context id.
    pub(super) fn context_id() -> [u8; 32] {
        env::context_id()
    }

    /// Return the executor id.
    pub(super) fn executor_id() -> [u8; 32] {
        env::executor_id()
    }

    /// Prints the log
    pub(super) fn log(message: &str) {
        env::log(message);
    }

    /// Gets the current time.
    ///
    /// This function obtains the current time as a nanosecond timestamp.
    ///
    pub(super) fn time_now() -> u64 {
        env::time_now()
    }

    /// Verifies an Ed25519 signature.
    pub(super) fn ed25519_verify(
        signature: &[u8; 64],
        public_key: &[u8; 32],
        message: &[u8],
    ) -> bool {
        // Call the host function from the calimero_sdk
        calimero_sdk::env::ed25519_verify(signature, public_key, message)
    }

    /// Get a new hybrid timestamp from the HLC
    pub(super) fn hlc_timestamp() -> HybridTimestamp {
        ensure_hlc_initialized();
        WASM_HLC.with(|hlc_cell| {
            hlc_cell
                .borrow_mut()
                .as_mut()
                .unwrap()
                .new_timestamp(env::time_now)
        })
    }

    /// Update HLC with remote timestamp
    pub(super) fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ()> {
        ensure_hlc_initialized();
        WASM_HLC.with(|hlc_cell| {
            hlc_cell
                .borrow_mut()
                .as_mut()
                .unwrap()
                .update(remote_ts, env::time_now)
        })
    }

    /// Resets the environment state for testing.
    #[cfg(test)]
    pub(super) fn reset_for_testing() {
        WASM_HLC.with(|hlc| {
            *hlc.borrow_mut() = None;
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod mocked {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};
    use std::cell::RefCell;
    use std::time::{SystemTime, UNIX_EPOCH};

    use rand::RngCore;

    use super::RuntimeEnv;
    use crate::logical_clock::{HybridTimestamp, LogicalClock};
    use crate::store::{Key, MockedStorage, StorageAdaptor};

    thread_local! {
        static ROOT_HASH: RefCell<Option<[u8; 32]>> = const { RefCell::new(None) };
        static NATIVE_HLC: RefCell<LogicalClock> = RefCell::new(LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf)));
        static RUNTIME_ENV: RefCell<Option<RuntimeEnv>> = const { RefCell::new(None) };
    }

    /// The default storage system.
    type DefaultStore = MockedStorage<{ usize::MAX }>;

    /// Commits the root hash to the runtime.
    pub(super) fn commit(root_hash: &[u8; 32], _artifact: &[u8]) {
        ROOT_HASH.with(|rh| {
            let _ = rh.borrow_mut().replace(*root_hash);
        });
    }

    /// Reads data from persistent storage.
    pub(super) fn storage_read(key: Key) -> Option<Vec<u8>> {
        let runtime_env = RUNTIME_ENV.with(|env| env.borrow().clone());
        if let Some(env) = runtime_env {
            let reader = env.storage_read();
            reader(&key)
        } else {
            DefaultStore::storage_read(key)
        }
    }

    /// Removes data from persistent storage.
    pub(super) fn storage_remove(key: Key) -> bool {
        let runtime_env = RUNTIME_ENV.with(|env| env.borrow().clone());
        if let Some(env) = runtime_env {
            let remover = env.storage_remove();
            remover(&key)
        } else {
            DefaultStore::storage_remove(key)
        }
    }

    /// Writes data to persistent storage.
    pub(super) fn storage_write(key: Key, value: &[u8]) -> bool {
        let runtime_env = RUNTIME_ENV.with(|env| env.borrow().clone());
        if let Some(env) = runtime_env {
            let writer = env.storage_write();
            writer(key, value)
        } else {
            DefaultStore::storage_write(key, value)
        }
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        rand::thread_rng().fill_bytes(buf);
    }

    /// Return the context id.
    pub(super) fn context_id() -> [u8; 32] {
        RUNTIME_ENV
            .with(|env| env.borrow().clone())
            .map(|env| env.context_id())
            .unwrap_or([236; 32])
    }

    thread_local! {
        static EXECUTOR_ID: std::cell::Cell<[u8; 32]> = const { std::cell::Cell::new([237; 32]) };
    }

    /// Return the executor id (for testing, returns a fixed value).
    pub(super) fn executor_id() -> [u8; 32] {
        RUNTIME_ENV
            .with(|env| env.borrow().clone())
            .map(|env| env.executor_id)
            .unwrap_or_else(|| EXECUTOR_ID.with(|id| id.get()))
    }

    /// Prints the log
    pub(super) fn log(message: &str) {
        println!("{}", message);
    }

    /// Set executor ID for testing purposes
    #[cfg(test)]
    pub(super) fn set_executor_id(new_id: [u8; 32]) {
        EXECUTOR_ID.with(|id| id.set(new_id));
    }

    /// Gets the current time.
    ///
    /// This function obtains the current time as a nanosecond timestamp.
    ///
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Impossible to overflow in normal circumstances"
    )]
    #[expect(clippy::expect_used, reason = "Effectively infallible here")]
    pub(super) fn time_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards to before the Unix epoch!")
            .as_nanos() as u64
    }

    /// Verifies an Ed25519 signature.
    ///
    /// Uses a pure-Rust implementation for testing.
    pub(super) fn ed25519_verify(
        signature: &[u8; 64],
        public_key: &[u8; 32],
        message: &[u8],
    ) -> bool {
        // We need to parse the public key.
        // If parsing fails, the signature is invalid.
        let Ok(public_key) = VerifyingKey::from_bytes(public_key) else {
            return false;
        };

        let signature = Signature::from_bytes(signature);
        // Perform the verification.
        public_key.verify(message, &signature).is_ok()
    }

    /// Get a new hybrid timestamp from the HLC
    pub(super) fn hlc_timestamp() -> HybridTimestamp {
        NATIVE_HLC.with(|hlc| hlc.borrow_mut().new_timestamp(time_now))
    }

    /// Update HLC with remote timestamp
    pub(super) fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ()> {
        NATIVE_HLC.with(|hlc| hlc.borrow_mut().update(remote_ts, time_now))
    }

    pub(super) fn with_runtime_env<R>(env: RuntimeEnv, f: impl FnOnce() -> R) -> R {
        RUNTIME_ENV.with(|slot| {
            let prev = slot.replace(Some(env));
            let result = f();
            slot.replace(prev);
            result
        })
    }

    /// Resets the environment state for testing.
    ///
    /// Clears the thread-local ROOT_HASH, HLC, and STORAGE, allowing multiple tests
    /// to run in sequence without contaminating each other.
    #[cfg(test)]
    pub(super) fn reset_for_testing() {
        ROOT_HASH.with(|rh| {
            *rh.borrow_mut() = None;
        });
        NATIVE_HLC.with(|hlc| {
            *hlc.borrow_mut() = LogicalClock::new(|buf| rand::thread_rng().fill_bytes(buf));
        });
        // Reset executor ID to default
        EXECUTOR_ID.with(|id| id.set([237; 32]));
        // Clear the mock storage to prevent test contamination
        crate::store::mocked::STORAGE.with(|storage| {
            storage.borrow_mut().clear();
        });
    }
}
