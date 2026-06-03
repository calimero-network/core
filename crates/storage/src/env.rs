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
///
/// **Re-entrant.** Restores the *prior* flag value on exit rather than
/// unconditionally clearing it. This matters when an outer scope already
/// holds merge mode (e.g. the `#[app::migrate]` macro wraps the whole
/// migrate body) and an inner storage op opens its own `with_merge_mode`
/// (the CRDT merge dispatch in `interface.rs`/`merge.rs`): an
/// unconditional `set(false)` on the inner exit would silently clear
/// merge mode for the *remainder of the outer body*, so any trailing
/// `LwwRegister::new()` (e.g. `total: count.into()` in a migrate) would
/// then bake a node-local HLC + executor_id into the serialised state and
/// diverge across nodes. The restore-on-exit (incl. unwind) keeps nesting
/// correct.
pub fn with_merge_mode<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            MERGE_MODE.with(|m| m.set(self.0));
        }
    }

    let _restore = Restore(MERGE_MODE.with(|m| m.replace(true)));
    f()
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

/// Returns the root hash recorded by the most recent native `commit` (test use).
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn root_hash() -> Option<[u8; 32]> {
    mocked::root_hash()
}

/// Returns (and clears) the `StorageDelta` artifact from the most recent native
/// `commit` (test use).
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn take_last_artifact() -> Option<Vec<u8>> {
    mocked::take_last_artifact()
}

/// Returns the raw bytes of the committed root `Entry` (the `Root<T>` slot)
/// from the native mock, or `None` if nothing has been committed yet.
///
/// Native/test use only. Application state commits to this mock, while
/// `calimero_sdk::read_raw()` reads a *separate* SDK-level host map. The
/// in-process test harness uses this to mirror the committed root across so a
/// `#[app::migrate]` body run under `TestHost` can observe the pre-migration
/// state. The bytes are the full `Entry<T>` (`borsh(T)` followed by the 32-byte
/// `Element.id`), matching what `read_raw` strips.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn read_committed_root_entry() -> Option<Vec<u8>> {
    storage_read(Key::Entry(crate::collections::ROOT_ENTRY_ID))
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

// === Ordered secondary index (SortedMap, core#2559) ===
//
// Raw-byte ordered keyspace (the backend keeps keys sorted, so a range scan is
// a native seek). Keys are the unhashed `collection ‖ order_key`. Node-local,
// NOT synced. Only the `MainStorage` adaptor routes here; `PrivateStorage` and
// the test mocks have their own index handling.

/// Insert/overwrite `key -> value` in the ordered index. Returns whether the
/// backend persisted the write (so `SortedMap` can skip stamping a stale
/// validity marker and rebuild on the next read instead).
#[must_use]
pub fn storage_index_set(key: &[u8], value: &[u8]) -> bool {
    imp::storage_index_set(key, value)
}

/// Remove `key` from the ordered index. Returns whether the write was
/// persisted (see [`storage_index_set`]).
#[must_use]
pub fn storage_index_remove(key: &[u8]) -> bool {
    imp::storage_index_remove(key)
}

/// Remove every ordered-index key beginning with `prefix`. Returns whether the
/// write was persisted (see [`storage_index_set`]).
#[must_use]
pub fn storage_index_remove_prefix(prefix: &[u8]) -> bool {
    imp::storage_index_remove_prefix(prefix)
}

/// Scan the ordered index over `[lo, hi)`, ascending, after `offset`, capped at
/// `limit` (`None` = unbounded). Returns `(key, value)` pairs.
#[must_use]
pub fn storage_index_scan(
    lo: &[u8],
    hi: &[u8],
    offset: usize,
    limit: Option<usize>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    imp::storage_index_scan(lo, hi, offset, limit)
}

/// The largest `(key, value)` in the ordered index over `[lo, hi)` (reverse
/// seek; backs `SortedMap::last`).
#[must_use]
pub fn storage_index_last(lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    imp::storage_index_last(lo, hi)
}

/// Reads data from node-local (private) persistent storage.
///
/// Private storage is **NOT synchronised across nodes** — entries
/// written here stay on this node only. Used by the `PrivateStorage`
/// adaptor that backs `#[app::private]` collections.
#[must_use]
pub fn private_storage_read(key: Key) -> Option<Vec<u8>> {
    imp::private_storage_read(key)
}

/// Removes data from node-local (private) persistent storage.
#[must_use]
pub fn private_storage_remove(key: Key) -> bool {
    imp::private_storage_remove(key)
}

/// Writes data to node-local (private) persistent storage.
#[must_use]
pub fn private_storage_write(key: Key, value: &[u8]) -> bool {
    imp::private_storage_write(key, value)
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

/// Resets all native (mocked) host state: in-memory storage, root hash,
/// HLC, and executor identity.
///
/// This is the public entry point used by the in-process test harness
/// (`calimero_sdk::testing::TestHost`) to isolate state between harness
/// instances created on the same thread. Native-only: the WASM host owns
/// real storage and there is nothing to reset there.
#[cfg(not(target_arch = "wasm32"))]
pub fn reset_environment() {
    mocked::reset_environment();
}

/// Set executor ID. `pub(crate)` because the only sanctioned way to mutate
/// executor identity from outside the crate is the scoped [`with_executor_id`]
/// guard below — that guard guarantees restoration on panic, whereas a raw
/// setter would leave a thread polluted on unwind.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_executor_id(id: [u8; 32]) {
    imp::set_executor_id(id);
}

/// Run `f` with the executor identity set to `id`, then restore the prior
/// identity (whatever it was) when the closure returns — even on panic.
///
/// Integration tests for CRDTs frequently need to simulate writes from several
/// different authors against the same in-process replica. The closure form
/// makes the save/restore pairing impossible to forget and unwind-safe via the
/// inner RAII guard: a panicking test still cleans up the thread-local before
/// the next test runs.
///
/// # Scope of effect
///
/// Only writes the `EXECUTOR_ID` thread-local. If a [`with_runtime_env`]-style
/// `RuntimeEnv` is installed when `with_executor_id` is called, the public
/// [`executor_id`] getter will continue to return the `RuntimeEnv`'s identity
/// (it prefers `RuntimeEnv` over the thread-local), so the guard's `id` is
/// effectively shadowed for the duration of `f()`. Tests that need to override
/// identity must not be nested inside a `RuntimeEnv`; the contract tests in
/// this crate use the plain thread-local path and are unaffected.
///
/// Native-only: WASM doesn't expose executor-identity mutation (the runtime
/// owns it). The `#[cfg(not(target_arch = "wasm32"))]` gate matches
/// [`set_executor_id`].
#[cfg(not(target_arch = "wasm32"))]
pub fn with_executor_id<R>(id: [u8; 32], f: impl FnOnce() -> R) -> R {
    struct Guard {
        prior: [u8; 32],
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            set_executor_id(self.prior);
        }
    }

    // Save and restore via the EXECUTOR_ID thread-local rather than the
    // public `executor_id()` getter: that getter prefers a `RuntimeEnv`
    // value when one is installed, but `set_executor_id` only writes
    // the thread-local fallback — so reading via `executor_id()` and
    // restoring via `set_executor_id` would be asymmetric. Anchoring
    // both ends on the same storage keeps the guard semantically
    // correct regardless of whether a runtime env is in scope.
    let prior = imp::executor_id_fallback();
    set_executor_id(id);
    let _g = Guard { prior };
    f()
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

    /// Reads data from node-local private storage.
    pub(super) fn private_storage_read(key: Key) -> Option<Vec<u8>> {
        env::private_storage_read(&key.to_bytes())
    }

    /// Removes data from node-local private storage.
    pub(super) fn private_storage_remove(key: Key) -> bool {
        env::private_storage_remove(&key.to_bytes())
    }

    /// Writes data to node-local private storage.
    pub(super) fn private_storage_write(key: Key, value: &[u8]) -> bool {
        env::private_storage_write(&key.to_bytes(), value)
    }

    /// Ordered-index ops (raw composite keys, no hashing — order must survive).
    pub(super) fn storage_index_set(key: &[u8], value: &[u8]) -> bool {
        env::storage_index_set(key, value)
    }

    pub(super) fn storage_index_remove(key: &[u8]) -> bool {
        env::storage_index_remove(key)
    }

    pub(super) fn storage_index_remove_prefix(prefix: &[u8]) -> bool {
        env::storage_index_remove_prefix(prefix)
    }

    pub(super) fn storage_index_scan(
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        env::storage_index_scan(lo, hi, offset, limit)
    }

    pub(super) fn storage_index_last(lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        env::storage_index_last(lo, hi)
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
        static LAST_ARTIFACT: RefCell<Option<Vec<u8>>> = const { RefCell::new(None) };
    }

    /// The default storage system.
    type DefaultStore = MockedStorage<{ usize::MAX }>;
    /// Scope used to back the mocked private-storage path. Distinct
    /// from `DefaultStore` so test-mode reads/writes through the
    /// `PrivateStorage` adaptor stay isolated from main-storage state
    /// — matching the WASM host's behaviour where private storage is
    /// a separate namespace.
    type DefaultPrivateStore = MockedStorage<{ usize::MAX - 1 }>;

    /// Commits the root hash to the runtime.
    pub(super) fn commit(root_hash: &[u8; 32], artifact: &[u8]) {
        ROOT_HASH.with(|rh| {
            let _ = rh.borrow_mut().replace(*root_hash);
        });
        LAST_ARTIFACT.with(|a| {
            *a.borrow_mut() = Some(artifact.to_vec());
        });
    }

    /// Returns the root hash recorded by the most recent [`commit`].
    pub(super) fn root_hash() -> Option<[u8; 32]> {
        ROOT_HASH.with(|rh| *rh.borrow())
    }

    /// Returns (and clears) the artifact recorded by the most recent [`commit`].
    pub(super) fn take_last_artifact() -> Option<Vec<u8>> {
        LAST_ARTIFACT.with(|a| a.borrow_mut().take())
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

    // Native ordered-index backend. A process-local `BTreeMap` standing in for
    // the node's RocksDB `SortedIndex` column — enough for native tests; the
    // node provides the durable, cross-run backing once wired. Keys are the raw
    // composite `collection ‖ order_key`, so `BTreeMap` order == key order.
    thread_local! {
        static INDEX: RefCell<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> =
            const { RefCell::new(std::collections::BTreeMap::new()) };
    }

    pub(super) fn storage_index_set(key: &[u8], value: &[u8]) -> bool {
        INDEX.with(|index| {
            let _ = index.borrow_mut().insert(key.to_vec(), value.to_vec());
        });
        true
    }

    pub(super) fn storage_index_remove(key: &[u8]) -> bool {
        INDEX.with(|index| {
            let _ = index.borrow_mut().remove(key);
        });
        true
    }

    pub(super) fn storage_index_remove_prefix(prefix: &[u8]) -> bool {
        INDEX.with(|index| index.borrow_mut().retain(|k, _| !k.starts_with(prefix)));
        true
    }

    pub(super) fn storage_index_scan(
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        INDEX.with(|index| {
            let matched: Vec<(Vec<u8>, Vec<u8>)> = index
                .borrow()
                .range(lo.to_vec()..hi.to_vec())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let ordered = matched.into_iter().skip(offset);
            match limit {
                Some(n) => ordered.take(n).collect(),
                None => ordered.collect(),
            }
        })
    }

    pub(super) fn storage_index_last(lo: &[u8], hi: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        INDEX.with(|index| {
            index
                .borrow()
                .range(lo.to_vec()..hi.to_vec())
                .next_back()
                .map(|(k, v)| (k.clone(), v.clone()))
        })
    }

    // Why these don't consult `RUNTIME_ENV` like their main-storage
    // siblings:
    //
    // `RuntimeEnv` carries callbacks only for main-storage reads /
    // writes / removes (see `super::RuntimeEnv`) — it has no private
    // storage backend to route to. That's not an omission: in
    // production, private storage is served by a dedicated WASM host
    // import (`imp::private_storage_*` → `VMLogic::private_storage`
    // → a separate `Storage` handle that maps to its own RocksDB
    // column, see `crates/context/src/handlers/execute/storage.rs`'s
    // `ContextPrivateStorage`). `with_runtime_env` is only installed
    // around native shim code that drives MainStorage (snapshot /
    // signature persistence in `crates/context` and `crates/runtime`)
    // — none of those scopes touch private storage.
    //
    // So in mocked mode, `DefaultPrivateStore` IS the backend for
    // node-local private state. The asymmetry vs `storage_*` is
    // intentional: there is nothing else to route to. If a future
    // caller ever needs runtime-env routing for private state (e.g. a
    // native test harness that wants reads/writes to land in a real
    // `Storage` handle), the right fix is to extend `RuntimeEnv` with
    // private callbacks rather than re-pointing this mock — the
    // current contract is "private storage is per-node-local; in
    // tests, the mock IS the node."

    /// Reads data from node-local private storage. Mocked path routes
    /// to a separate `MockedStorage` scope so private state stays
    /// isolated from main state in tests, matching the WASM host's
    /// separate-namespace behaviour.
    pub(super) fn private_storage_read(key: Key) -> Option<Vec<u8>> {
        DefaultPrivateStore::storage_read(key)
    }

    /// Removes data from node-local private storage (mocked path).
    pub(super) fn private_storage_remove(key: Key) -> bool {
        DefaultPrivateStore::storage_remove(key)
    }

    /// Writes data to node-local private storage (mocked path).
    pub(super) fn private_storage_write(key: Key, value: &[u8]) -> bool {
        DefaultPrivateStore::storage_write(key, value)
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

    /// Sets the thread-local executor ID. Only callable from this crate
    /// via the `pub(crate)` re-export above; external callers must go
    /// through the scoped [`super::with_executor_id`] guard so they
    /// can't forget to restore prior state on panic.
    pub(super) fn set_executor_id(new_id: [u8; 32]) {
        EXECUTOR_ID.with(|id| id.set(new_id));
    }

    /// Reads the thread-local executor ID fallback, bypassing
    /// `RUNTIME_ENV`. Used by [`super::with_executor_id`] for symmetric
    /// save/restore around its mutation of the same thread-local — the
    /// public `executor_id()` getter prefers `RUNTIME_ENV`, which
    /// wouldn't restore correctly via `set_executor_id`.
    pub(super) fn executor_id_fallback() -> [u8; 32] {
        EXECUTOR_ID.with(|id| id.get())
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
    pub(super) fn reset_environment() {
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
        // Clear the native ordered-index mock too.
        INDEX.with(|index| index.borrow_mut().clear());
        LAST_ARTIFACT.with(|a| {
            *a.borrow_mut() = None;
        });
    }

    /// Resets the environment state for testing (legacy `#[cfg(test)]` alias).
    #[cfg(test)]
    pub(super) fn reset_for_testing() {
        reset_environment();
    }
}
