//! Environment bindings for the storage crate.

#[cfg(target_arch = "wasm32")]
use calimero_vm as imp;
#[cfg(not(target_arch = "wasm32"))]
use mocked as imp;

use std::cell::Cell;

use crate::logical_clock::{ClockUpdateError, HybridTimestamp};
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
/// then bake a node-local HLC + device_id into the serialised state and
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
/// Reference-counted host callback for reading a key's stored bytes.
type StorageReadFn = std::rc::Rc<dyn Fn(&Key) -> Option<Vec<u8>>>;
#[cfg(not(target_arch = "wasm32"))]
/// Reference-counted host callback for writing a key's bytes.
type StorageWriteFn = std::rc::Rc<dyn Fn(Key, &[u8]) -> bool>;
#[cfg(not(target_arch = "wasm32"))]
/// Reference-counted host callback for removing a key.
type StorageRemoveFn = std::rc::Rc<dyn Fn(&Key) -> bool>;

// === Ordered-index host callbacks (SortedMap/SortedSet, core#2559) ===
//
// The node-local ordered index and its validity marker live in dedicated,
// non-synced columns (`Column::SortedIndex` / `Column::SortedIndexMeta`). On a
// WASM guest those are reached through the `storage_index_*` host imports →
// `ContextStorage`. But `SortedSet`/`SortedMap` also run **host-side, natively**
// (the JS SDK drives `js_crdt_sortedset_*` → `SortedSet::<MainStorage>::iter`
// inside `merod`, and delta/HashComparison apply runs `Interface::apply_action`
// natively). On that native path `MainStorage`'s index ops go through
// `calimero_storage::env`, which without these callbacks falls back to a
// process-thread-local mock — NOT the durable, context-scoped RocksDB columns.
// That split let a JS `SortedSet`'s ordered read and its sync-apply target
// different stores, so a converged set stayed stale on the ordered readers
// (sdk-js#87).
//
// These callbacks bridge the native index ops to the SAME host store the guest
// path uses (`ContextStorage` under the runtime, the RocksDB `SortedIndex`/
// `SortedIndexMeta` columns under the node's sync bridge), so every native
// SortedSet/SortedMap read and every native apply-time marker clear share one
// durable, cross-thread-coherent store. Keys are the adaptor-level
// `collection ‖ order_key` (meta: `collection_id`); the bridge implementation
// applies whatever context scoping the underlying store needs.
#[cfg(not(target_arch = "wasm32"))]
type IndexWriteFn = std::rc::Rc<dyn Fn(&[u8], &[u8]) -> bool>;
#[cfg(not(target_arch = "wasm32"))]
type IndexKeyFn = std::rc::Rc<dyn Fn(&[u8]) -> bool>;
#[cfg(not(target_arch = "wasm32"))]
type IndexScanFn =
    std::rc::Rc<dyn Fn(&[u8], &[u8], usize, Option<usize>) -> Vec<(Vec<u8>, Vec<u8>)>>;
#[cfg(not(target_arch = "wasm32"))]
type IndexSeekFn = std::rc::Rc<dyn Fn(&[u8], &[u8]) -> Option<(Vec<u8>, Vec<u8>)>>;
#[cfg(not(target_arch = "wasm32"))]
type IndexReadFn = std::rc::Rc<dyn Fn(&[u8]) -> Option<Vec<u8>>>;

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
/// Host callbacks routing the node-local ordered index + validity marker to the
/// real host store (see the module comment above). Installed on a [`RuntimeEnv`]
/// via [`RuntimeEnv::with_index`]; when absent, the native ordered-index ops use
/// the process-thread-local mock (fine for pure `calimero-storage` unit tests,
/// wrong for a real node running host-side SortedSet/SortedMap).
pub struct IndexCallbacks {
    /// `collection ‖ order_key -> entry_id` insert/overwrite.
    pub set: IndexWriteFn,
    /// Remove one `collection ‖ order_key`.
    pub remove: IndexKeyFn,
    /// Remove every key beginning with `prefix` (a `collection_id`).
    pub remove_prefix: IndexKeyFn,
    /// Ascending `[lo, hi)` scan after `offset`, capped at `limit`.
    pub scan: IndexScanFn,
    /// Largest `(key, value)` in `[lo, hi)` (reverse seek for `last`).
    pub last: IndexSeekFn,
    /// Write the validity marker for `collection_id`.
    pub meta_set: IndexWriteFn,
    /// Read the validity marker for `collection_id`.
    pub meta_get: IndexReadFn,
    /// Clear the validity marker for `collection_id` (invalidate-on-sync).
    pub meta_clear: IndexKeyFn,
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
///
/// `index` optionally routes the node-local ordered index + marker to the same
/// host store (see [`IndexCallbacks`]); when `None`, those ops use the
/// process-thread-local mock.
pub struct RuntimeEnv {
    storage_read: StorageReadFn,
    storage_write: StorageWriteFn,
    storage_remove: StorageRemoveFn,
    context_id: [u8; 32],
    device_id: [u8; 32],
    account_id: [u8; 32],
    index: Option<IndexCallbacks>,
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
        storage_read: StorageReadFn,
        storage_write: StorageWriteFn,
        storage_remove: StorageRemoveFn,
        context_id: [u8; 32],
        device_id: [u8; 32],
        account_id: [u8; 32],
    ) -> Self {
        Self {
            storage_read,
            storage_write,
            storage_remove,
            context_id,
            device_id,
            account_id,
            index: None,
        }
    }

    #[must_use]
    /// Attaches ordered-index host callbacks so native `SortedSet`/`SortedMap`
    /// index + marker ops reach the real host store instead of the
    /// process-thread-local mock (see [`IndexCallbacks`]). Consuming builder so
    /// the common [`new`](Self::new) path stays unchanged for callers that don't
    /// drive host-side ordered collections.
    pub fn with_index(mut self, index: IndexCallbacks) -> Self {
        self.index = Some(index);
        self
    }

    #[must_use]
    /// Returns the installed ordered-index callbacks, if any.
    pub fn index(&self) -> Option<IndexCallbacks> {
        self.index.clone()
    }

    #[must_use]
    /// Returns the storage read callback.
    pub fn storage_read(&self) -> StorageReadFn {
        self.storage_read.clone()
    }

    #[must_use]
    /// Returns the storage write callback.
    pub fn storage_write(&self) -> StorageWriteFn {
        self.storage_write.clone()
    }

    #[must_use]
    /// Returns the storage remove callback.
    pub fn storage_remove(&self) -> StorageRemoveFn {
        self.storage_remove.clone()
    }

    #[must_use]
    /// Returns the current context identifier.
    pub const fn context_id(&self) -> [u8; 32] {
        self.context_id
    }

    #[must_use]
    /// Returns the current executor identifier.
    pub const fn device_id(&self) -> [u8; 32] {
        self.device_id
    }

    #[must_use]
    /// Returns the account this execution is authorized as — the writer-set
    /// principal, resolved by the node from the executing device's binding.
    pub const fn account_id(&self) -> [u8; 32] {
        self.account_id
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

/// Write the node-local ordered-index validity marker `key -> value`. Node-local
/// and NOT synced (it lives beside the index it guards, never in synced state).
/// Returns whether the write was persisted.
#[must_use]
pub fn storage_index_meta_set(key: &[u8], value: &[u8]) -> bool {
    imp::storage_index_meta_set(key, value)
}

/// Read the node-local ordered-index validity marker for `key` (see
/// [`storage_index_meta_set`]).
#[must_use]
pub fn storage_index_meta_get(key: &[u8]) -> Option<Vec<u8>> {
    imp::storage_index_meta_get(key)
}

/// Clear the node-local ordered-index validity marker for `key` (a
/// `collection_id`), so the next ordered read rebuilds the index. This is the
/// invalidate-on-sync primitive routed to the same non-synced marker column as
/// [`storage_index_meta_set`]. Returns whether the write was persisted.
#[must_use]
pub fn storage_index_meta_clear(key: &[u8]) -> bool {
    imp::storage_index_meta_clear(key)
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
pub fn context_id() -> [u8; 32] {
    imp::context_id()
}

/// Returns the id of the **device** executing right now — the replica this crate
/// attributes writes to.
///
/// Every use of it in this crate is a per-replica question: an LWW register's
/// tiebreak `node_id`, a counter's per-actor slot, an HLC instance seed, an
/// author-tracked entry's owner. None of those may become an account: two devices
/// of one account acting concurrently must stay distinguishable, or they share a
/// counter slot and an HLC seed and silently lose each other's writes. Per-person
/// aggregation belongs above storage, on `account_id()`.
///
/// In WASM, this calls the host function. In tests, returns a fixed value.
#[must_use]
pub fn device_id() -> [u8; 32] {
    imp::device_id()
}

/// The **account** this execution is authorized as — the writer-set principal.
///
/// The counterpart to [`device_id`], and the one that belongs in an access-control
/// set: a person with several devices is one account, so granting them does not
/// mean enumerating their machines. Never use it where per-replica uniqueness is
/// required; see [`device_id`] for that list.
#[must_use]
pub fn account_id() -> [u8; 32] {
    imp::account_id()
}

/// Prints the log.
///
/// In WASM, this calls `calimero_sdk::env::log()`, which calls the host function.
/// In tests, it uses plain `println!()`.
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
/// Returns a [`ClockUpdateError`] describing why the remote timestamp was
/// rejected — currently [`ClockUpdateError::Drift`] when it is more than 5s in
/// the future (drift protection). The typed reason is preserved from the clock
/// itself rather than being flattened into an opaque failure.
pub fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ClockUpdateError> {
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

/// Clears ONLY the native node-local ordered index (and its validity markers),
/// leaving synced state intact. Test-only seam for reproducing a node that has
/// converged state (via sync) but an unbuilt/stale node-local `SortedMap`
/// index — the marker must then be node-local so the next ordered read rebuilds
/// rather than trusting a peer-derived "index current" signal. Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn clear_sorted_index_for_testing() {
    mocked::clear_sorted_index_for_testing();
}

/// Drops the single node-local ordered-index entry for `order_key`, leaving the
/// index's validity marker and all synced state intact. Test-only seam for
/// reproducing a node whose ordered index holds a stale subset of the converged
/// element set while its marker still equals the converged `full_hash` — the
/// false positive an ordered read must detect and rebuild past (sdk-js#87).
/// Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn drop_sorted_index_entry_for_testing(order_key: &[u8]) {
    mocked::drop_sorted_index_entry_for_testing(order_key);
}

/// Set executor ID. `pub(crate)` because the only sanctioned way to mutate
/// executor identity from outside the crate is the scoped [`with_device_id`]
/// guard below — that guard guarantees restoration on panic, whereas a raw
/// setter would leave a thread polluted on unwind.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn set_device_id(id: [u8; 32]) {
    imp::set_device_id(id);
}

/// Set the account ID — the writer-set principal. `pub(crate)` for the same
/// reason as [`set_device_id`]: outside callers use the scoped
/// [`with_account_id`] guard, which restores on panic.
#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "in-crate tests are the only callers; the `testing`-feature build                   compiles the same file without cfg(test), where they are invisible"
    )
)]
pub(crate) fn set_account_id(id: [u8; 32]) {
    imp::set_account_id(id);
}

/// Run `f` as `account`, then restore the prior account — even on panic.
///
/// The account is what a writer set grants, so this is how a test says "the same
/// person, from a different device": hold the device fixed and move the account,
/// or hold the account and move the device with [`with_device_id`]. Moving both
/// at once cannot distinguish an account-keyed gate from a device-keyed one.
#[cfg(not(target_arch = "wasm32"))]
pub fn with_account_id<T>(account: [u8; 32], f: impl FnOnce() -> T) -> T {
    struct Restore([u8; 32]);
    impl Drop for Restore {
        fn drop(&mut self) {
            imp::set_account_id(self.0);
        }
    }
    let _restore = Restore(imp::account_id_fallback());
    imp::set_account_id(account);
    f()
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
/// `RuntimeEnv` is installed when `with_device_id` is called, the public
/// [`device_id`] getter will continue to return the `RuntimeEnv`'s identity
/// (it prefers `RuntimeEnv` over the thread-local), so the guard's `id` is
/// effectively shadowed for the duration of `f()`. Tests that need to override
/// identity must not be nested inside a `RuntimeEnv`; the contract tests in
/// this crate use the plain thread-local path and are unaffected.
///
/// Native-only: WASM doesn't expose executor-identity mutation (the runtime
/// owns it). The `#[cfg(not(target_arch = "wasm32"))]` gate matches
/// [`set_device_id`].
#[cfg(not(target_arch = "wasm32"))]
pub fn with_device_id<R>(id: [u8; 32], f: impl FnOnce() -> R) -> R {
    struct Guard {
        prior: [u8; 32],
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            set_device_id(self.prior);
        }
    }

    // Save and restore via the EXECUTOR_ID thread-local rather than the
    // public `device_id()` getter: that getter prefers a `RuntimeEnv`
    // value when one is installed, but `set_device_id` only writes
    // the thread-local fallback — so reading via `device_id()` and
    // restoring via `set_device_id` would be asymmetric. Anchoring
    // both ends on the same storage keeps the guard semantically
    // correct regardless of whether a runtime env is in scope.
    let prior = imp::device_id_fallback();
    set_device_id(id);
    let _g = Guard { prior };
    f()
}

#[cfg(target_arch = "wasm32")]
mod calimero_vm {
    use std::cell::RefCell;

    use calimero_sdk::env;

    use crate::logical_clock::{ClockUpdateError, HybridTimestamp, LogicalClock};
    use crate::store::Key;

    thread_local! {
        static WASM_HLC: RefCell<Option<LogicalClock>> = const { RefCell::new(None) };
    }

    fn ensure_hlc_initialized() {
        WASM_HLC.with(|hlc_cell| {
            if hlc_cell.borrow().is_none() {
                // Deterministic per-node HLC seed from the executor id, from all
                // 32 bytes via SHA-256 — using only the first 16 collapsed
                // executors sharing a 16-byte prefix to one id → CharId collision
                // → silent character loss during RGA sync.
                let device_id = env::device_id();
                let seed = crate::logical_clock::hlc_seed_from_device_id(&device_id);
                *hlc_cell.borrow_mut() = Some(LogicalClock::new(|buf| {
                    let n = buf.len().min(seed.len());
                    buf[..n].copy_from_slice(&seed[..n]);
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

    pub(super) fn storage_index_meta_set(key: &[u8], value: &[u8]) -> bool {
        env::storage_index_meta_set(key, value)
    }

    pub(super) fn storage_index_meta_get(key: &[u8]) -> Option<Vec<u8>> {
        env::storage_index_meta_get(key)
    }

    pub(super) fn storage_index_meta_clear(key: &[u8]) -> bool {
        env::storage_index_meta_clear(key)
    }

    /// Fills the buffer with random bytes.
    pub(super) fn random_bytes(buf: &mut [u8]) {
        env::random_bytes(buf)
    }

    /// Return the context id.
    pub(super) fn context_id() -> [u8; 32] {
        env::context_id()
    }

    /// Return the executing device id.
    pub(super) fn device_id() -> [u8; 32] {
        env::device_id()
    }

    /// Return the executing account id.
    pub(super) fn account_id() -> [u8; 32] {
        env::account_id()
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
    pub(super) fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ClockUpdateError> {
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
    use crate::logical_clock::{ClockUpdateError, HybridTimestamp, LogicalClock};
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
    // the node's RocksDB `SortedIndex` column — used for pure `calimero-storage`
    // unit tests, where no `RuntimeEnv` index bridge is installed. On a real node
    // an installed `RuntimeEnv::with_index` routes every op below to the durable,
    // context-scoped host store instead (see `IndexCallbacks`). Keys are the raw
    // composite `collection ‖ order_key`, so `BTreeMap` order == key order.
    thread_local! {
        static INDEX: RefCell<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> =
            const { RefCell::new(std::collections::BTreeMap::new()) };
        /// Node-local ordered-index validity markers, keyed by `collection_id`.
        /// A sibling of `INDEX` (never the synced state store), mirroring the
        /// RocksDB `SortedIndexMeta` column — so a marker is node-local exactly
        /// like the index it guards.
        static INDEX_META: RefCell<std::collections::BTreeMap<Vec<u8>, Vec<u8>>> =
            const { RefCell::new(std::collections::BTreeMap::new()) };
    }

    /// The ordered-index callbacks installed by the current `RuntimeEnv`, if any.
    /// When present, every native index op below routes to the host store rather
    /// than the process-thread-local `INDEX`/`INDEX_META` mock.
    fn index_bridge() -> Option<super::IndexCallbacks> {
        RUNTIME_ENV.with(|env| env.borrow().as_ref().and_then(super::RuntimeEnv::index))
    }

    pub(super) fn storage_index_set(key: &[u8], value: &[u8]) -> bool {
        if let Some(bridge) = index_bridge() {
            return (bridge.set)(key, value);
        }
        INDEX.with(|index| {
            let _ = index.borrow_mut().insert(key.to_vec(), value.to_vec());
        });
        true
    }

    pub(super) fn storage_index_remove(key: &[u8]) -> bool {
        if let Some(bridge) = index_bridge() {
            return (bridge.remove)(key);
        }
        INDEX.with(|index| {
            let _ = index.borrow_mut().remove(key);
        });
        true
    }

    pub(super) fn storage_index_remove_prefix(prefix: &[u8]) -> bool {
        if let Some(bridge) = index_bridge() {
            return (bridge.remove_prefix)(prefix);
        }
        INDEX.with(|index| index.borrow_mut().retain(|k, _| !k.starts_with(prefix)));
        true
    }

    pub(super) fn storage_index_scan(
        lo: &[u8],
        hi: &[u8],
        offset: usize,
        limit: Option<usize>,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        if let Some(bridge) = index_bridge() {
            return (bridge.scan)(lo, hi, offset, limit);
        }
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
        if let Some(bridge) = index_bridge() {
            return (bridge.last)(lo, hi);
        }
        INDEX.with(|index| {
            index
                .borrow()
                .range(lo.to_vec()..hi.to_vec())
                .next_back()
                .map(|(k, v)| (k.clone(), v.clone()))
        })
    }

    pub(super) fn storage_index_meta_set(key: &[u8], value: &[u8]) -> bool {
        if let Some(bridge) = index_bridge() {
            return (bridge.meta_set)(key, value);
        }
        INDEX_META.with(|meta| {
            let _ = meta.borrow_mut().insert(key.to_vec(), value.to_vec());
        });
        true
    }

    pub(super) fn storage_index_meta_get(key: &[u8]) -> Option<Vec<u8>> {
        if let Some(bridge) = index_bridge() {
            return (bridge.meta_get)(key);
        }
        INDEX_META.with(|meta| meta.borrow().get(key).cloned())
    }

    pub(super) fn storage_index_meta_clear(key: &[u8]) -> bool {
        if let Some(bridge) = index_bridge() {
            return (bridge.meta_clear)(key);
        }
        INDEX_META.with(|meta| {
            let _ = meta.borrow_mut().remove(key);
        });
        true
    }

    /// Clear ONLY the node-local ordered index and its validity markers, leaving
    /// state (entities) intact. Models a node that received converged state via
    /// sync but has not yet (re)built its node-local `SortedMap` index — the
    /// exact condition the marker's node-locality must handle correctly.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn clear_sorted_index_for_testing() {
        INDEX.with(|index| index.borrow_mut().clear());
        INDEX_META.with(|meta| meta.borrow_mut().clear());
    }

    /// Drop the single node-local ordered-index entry for `order_key` (matched
    /// as the suffix of the `collection_id ‖ order_key` composite), leaving the
    /// validity marker and all state untouched. Models a node whose index holds
    /// a stale subset of the converged element set while its marker still equals
    /// the converged `full_hash` — the false positive the rebuild trigger must
    /// catch (sdk-js#87).
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn drop_sorted_index_entry_for_testing(order_key: &[u8]) {
        INDEX.with(|index| {
            index
                .borrow_mut()
                .retain(|k, _| !(k.len() == 32 + order_key.len() && k.ends_with(order_key)));
        });
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
        /// Deliberately NOT equal to `EXECUTOR_ID`'s default — see `account_id`.
        static ACCOUNT_ID: std::cell::Cell<[u8; 32]> = const { std::cell::Cell::new([173; 32]) };
    }

    /// Return the executor id (for testing, returns a fixed value).
    pub(super) fn device_id() -> [u8; 32] {
        RUNTIME_ENV
            .with(|env| env.borrow().clone())
            .map(|env| env.device_id)
            .unwrap_or_else(|| EXECUTOR_ID.with(|id| id.get()))
    }

    /// Return the account id (for testing, a fixed value **distinct** from the
    /// device default).
    ///
    /// The two mock defaults must never be equal: a test that confuses the writer-set
    /// principal with the replica id would pass under identical defaults and fail only
    /// on a real node, where they are never the same value.
    pub(super) fn account_id() -> [u8; 32] {
        RUNTIME_ENV
            .with(|env| env.borrow().clone())
            .map(|env| env.account_id)
            .unwrap_or_else(|| ACCOUNT_ID.with(|id| id.get()))
    }

    /// Routes the log line through `tracing` (this is the host/native build, so
    /// a subscriber is present) rather than raw stdout, so guest/app logs carry
    /// the process's structured formatting and can be filtered and redirected
    /// like every other log instead of bypassing it onto stdout.
    pub(super) fn log(message: &str) {
        tracing::info!(target: "calimero_storage::guest", "{message}");
    }

    /// Sets the thread-local executor ID. Only callable from this crate
    /// via the `pub(crate)` re-export above; external callers must go
    /// through the scoped [`super::with_device_id`] guard so they
    /// can't forget to restore prior state on panic.
    pub(super) fn set_device_id(new_id: [u8; 32]) {
        EXECUTOR_ID.with(|id| id.set(new_id));
    }

    /// Sets the thread-local account ID — the writer-set principal.
    ///
    /// Separate from [`set_device_id`] on purpose: a test that moves the device
    /// and expects the gate to follow is testing the pre-account behaviour, and a
    /// test that moves both together cannot tell an account-keyed gate from a
    /// device-keyed one. Move them independently.
    pub(super) fn set_account_id(new_id: [u8; 32]) {
        ACCOUNT_ID.with(|id| id.set(new_id));
    }

    /// Reads the thread-local account ID fallback, bypassing `RUNTIME_ENV` —
    /// the symmetric partner of [`device_id_fallback`], for the scoped guard's
    /// save/restore.
    pub(super) fn account_id_fallback() -> [u8; 32] {
        ACCOUNT_ID.with(|id| id.get())
    }

    /// Reads the thread-local executor ID fallback, bypassing
    /// `RUNTIME_ENV`. Used by [`super::with_device_id`] for symmetric
    /// save/restore around its mutation of the same thread-local — the
    /// public `device_id()` getter prefers `RUNTIME_ENV`, which
    /// wouldn't restore correctly via `set_device_id`.
    pub(super) fn device_id_fallback() -> [u8; 32] {
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
    pub(super) fn update_hlc(remote_ts: &HybridTimestamp) -> Result<(), ClockUpdateError> {
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
        // Clear the native ordered-index mock too (entries + validity markers).
        INDEX.with(|index| index.borrow_mut().clear());
        INDEX_META.with(|meta| meta.borrow_mut().clear());
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
