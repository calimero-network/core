//! Native (non-`wasm32`) mock host backend for the in-process test harness.
//!
//! On `wasm32` every [`crate::env`] function forwards to a `calimero_sys`
//! import that the node's WASM runtime supplies. Off-`wasm32` those imports
//! don't exist (they `panic!` with "only available when compiled for wasm32"),
//! which is why app logic could never run under a plain `cargo test`.
//!
//! This module fills that gap: it keeps a thread-local [`MockHost`] that records
//! emitted events and logs, serves a configurable executor / context identity,
//! and provides an in-memory key/value store. [`crate::testing::TestHost`] drives
//! it so application methods can be exercised as ordinary Rust — no WASM build,
//! no containers.
//!
//! State storage of the *application* itself flows through `calimero_storage`'s
//! own native mock (it has a separate backend and can't depend back on the SDK).
//! The store here backs the SDK-level [`crate::env::storage_read`] /
//! `storage_write` surface (e.g. [`crate::state::read_raw`]) so raw-storage and
//! migration helpers don't trap during tests.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// Default context identity returned by the mock host.
///
/// Matches `calimero_storage`'s native `context_id()` default so the SDK and
/// storage layers agree out of the box when a test doesn't override identity.
pub(crate) const DEFAULT_CONTEXT_ID: [u8; 32] = [236; 32];

/// Default executor identity returned by the mock host.
///
/// Matches `calimero_storage`'s native `executor_id()` default for the same
/// reason as [`DEFAULT_CONTEXT_ID`].
pub(crate) const DEFAULT_EXECUTOR_ID: [u8; 32] = [237; 32];

/// An event captured by the mock host during a test.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct CapturedEvent {
    /// The event kind, as reported by `AppEvent::kind`.
    pub kind: String,
    /// The raw serialized event payload, as reported by `AppEvent::data`.
    pub data: Vec<u8>,
    /// The callback handler name, if the event was emitted via
    /// `emit_with_handler`; `None` for a plain `emit`.
    pub handler: Option<String>,
}

/// Thread-local mock host state.
struct MockHost {
    events: Vec<CapturedEvent>,
    logs: Vec<String>,
    context_id: [u8; 32],
    executor_id: [u8; 32],
    storage: BTreeMap<Vec<u8>, Vec<u8>>,
    private_storage: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Seed for the deterministic-enough PRNG backing `random_bytes`.
    rng_state: u64,
    /// Last value handed out by `time_now`, to keep timestamps strictly increasing.
    last_time: u64,
    /// Finalized blobs, keyed by their content hash.
    blobs: BTreeMap<[u8; 32], Vec<u8>>,
    /// Open write handles: file descriptor -> accumulated bytes.
    blob_write_handles: BTreeMap<u64, Vec<u8>>,
    /// Open read handles: file descriptor -> (blob id, data, cursor).
    blob_read_handles: BTreeMap<u64, ([u8; 32], Vec<u8>, usize)>,
    /// Next file descriptor to hand out. Starts at 1 so 0 stays "not found".
    next_fd: u64,
}

impl Default for MockHost {
    fn default() -> Self {
        Self {
            events: Vec::new(),
            logs: Vec::new(),
            context_id: DEFAULT_CONTEXT_ID,
            executor_id: DEFAULT_EXECUTOR_ID,
            storage: BTreeMap::new(),
            private_storage: BTreeMap::new(),
            rng_state: 0x9E37_79B9_7F4A_7C15,
            last_time: 0,
            blobs: BTreeMap::new(),
            blob_write_handles: BTreeMap::new(),
            blob_read_handles: BTreeMap::new(),
            next_fd: 1,
        }
    }
}

thread_local! {
    static HOST: RefCell<MockHost> = RefCell::new(MockHost::default());
}

fn with<R>(f: impl FnOnce(&mut MockHost) -> R) -> R {
    HOST.with(|h| f(&mut h.borrow_mut()))
}

// ============================================================================
// Harness controls (used by `crate::testing`)
// ============================================================================

/// Clears all recorded events, logs, storage, and buffered I/O and restores the
/// default identities. Called when a fresh [`crate::testing::TestHost`] is built.
pub(crate) fn reset() {
    with(|h| *h = MockHost::default());
}

/// Returns a clone of every event captured since the last [`reset`].
pub(crate) fn events() -> Vec<CapturedEvent> {
    with(|h| h.events.clone())
}

/// Removes and returns every event captured since the last [`reset`] or
/// [`take_events`].
pub(crate) fn take_events() -> Vec<CapturedEvent> {
    with(|h| std::mem::take(&mut h.events))
}

/// Returns a clone of every log line captured since the last [`reset`].
pub(crate) fn logs() -> Vec<String> {
    with(|h| h.logs.clone())
}

/// Removes and returns every log line captured since the last [`reset`] or
/// [`take_logs`].
pub(crate) fn take_logs() -> Vec<String> {
    with(|h| std::mem::take(&mut h.logs))
}

/// Overrides the executor identity the mock host reports to app logic.
pub(crate) fn set_executor_id(id: [u8; 32]) {
    with(|h| h.executor_id = id);
}

/// Overrides the context identity the mock host reports to app logic.
pub(crate) fn set_context_id(id: [u8; 32]) {
    with(|h| h.context_id = id);
}

/// Writes `value` directly into the SDK host storage map at `key`.
///
/// Application state is committed to `calimero_storage`'s own native mock, not
/// here; this lets the test harness mirror the committed root `Entry` into the
/// map that [`crate::state::read_raw`] reads, so a `#[app::migrate]` body run
/// under [`crate::testing::TestHost`] observes the pre-migration state.
pub(crate) fn seed_storage(key: &[u8], value: Vec<u8>) {
    with(|h| {
        let _ = h.storage.insert(key.to_vec(), value);
    });
}

// ============================================================================
// `env` host-function implementations
// ============================================================================

pub(crate) fn emit(kind: &str, data: &[u8]) {
    with(|h| {
        h.events.push(CapturedEvent {
            kind: kind.to_owned(),
            data: data.to_vec(),
            handler: None,
        });
    });
}

pub(crate) fn emit_with_handler(kind: &str, data: &[u8], handler: &str) {
    with(|h| {
        h.events.push(CapturedEvent {
            kind: kind.to_owned(),
            data: data.to_vec(),
            handler: Some(handler.to_owned()),
        });
    });
}

pub(crate) fn log(message: &str) {
    with(|h| h.logs.push(message.to_owned()));
}

pub(crate) fn context_id() -> [u8; 32] {
    with(|h| h.context_id)
}

pub(crate) fn executor_id() -> [u8; 32] {
    with(|h| h.executor_id)
}

pub(crate) fn xcall_origin() -> Option<[u8; 32]> {
    // `TestHost` drives methods directly rather than via xcall dispatch, so a
    // hosted call has no cross-context origin.
    None
}

pub(crate) fn input() -> Option<Vec<u8>> {
    // `TestHost` drives methods directly via closures rather than through the
    // JSON-input WASM entrypoint, so there is no input buffer to serve.
    None
}

pub(crate) fn value_return(_value: &[u8]) {
    // The harness reads a method's return value straight from the closure, so
    // the wire-format return channel is a no-op here.
}

pub(crate) fn emit_migration_witness(_blob: &[u8]) {
    // `TestHost` runs migrate/migration_check natively as plain functions, so the
    // witness flows as a real `(State, Witness)` return value / `check` argument
    // rather than through this wire channel. No-op here.
}

pub(crate) fn commit(_root_hash: &[u8; 32], _artifact: &[u8]) {
    // State is committed through `calimero_storage`'s own native mock; the SDK
    // commit hook has nothing to persist in-process.
}

pub(crate) fn storage_read(key: &[u8]) -> Option<Vec<u8>> {
    with(|h| h.storage.get(key).cloned())
}

pub(crate) fn storage_remove(key: &[u8]) -> bool {
    with(|h| h.storage.remove(key).is_some())
}

pub(crate) fn storage_write(key: &[u8], value: &[u8]) -> bool {
    with(|h| h.storage.insert(key.to_vec(), value.to_vec()).is_some())
}

pub(crate) fn private_storage_read(key: &[u8]) -> Option<Vec<u8>> {
    with(|h| h.private_storage.get(key).cloned())
}

pub(crate) fn private_storage_remove(key: &[u8]) -> bool {
    with(|h| h.private_storage.remove(key).is_some())
}

pub(crate) fn private_storage_write(key: &[u8], value: &[u8]) -> bool {
    with(|h| {
        let _ = h.private_storage.insert(key.to_vec(), value.to_vec());
    });
    // Match the WASM `private_storage_write` convention: `true` = the write
    // succeeded (false would mean private storage is unavailable on this node).
    // This deliberately differs from main `storage_write`, whose bool reports
    // whether a previous value was evicted — see `env::storage_write` /
    // `env::private_storage_write` docs.
    true
}

// ============================================================================
// Streaming blob API (in-memory)
// ============================================================================

pub(crate) fn blob_create() -> u64 {
    with(|h| {
        let fd = h.next_fd;
        h.next_fd += 1;
        h.blob_write_handles.insert(fd, Vec::new());
        fd
    })
}

pub(crate) fn blob_write(fd: u64, data: &[u8]) -> u64 {
    with(|h| {
        if let Some(buf) = h.blob_write_handles.get_mut(&fd) {
            buf.extend_from_slice(data);
            data.len() as u64
        } else {
            0
        }
    })
}

pub(crate) fn blob_open(blob_id: &[u8; 32]) -> u64 {
    with(|h| {
        let Some(data) = h.blobs.get(blob_id).cloned() else {
            return 0;
        };
        let fd = h.next_fd;
        h.next_fd += 1;
        h.blob_read_handles.insert(fd, (*blob_id, data, 0));
        fd
    })
}

pub(crate) fn blob_read(fd: u64, buffer: &mut [u8]) -> u64 {
    with(|h| {
        let Some((_, data, cursor)) = h.blob_read_handles.get_mut(&fd) else {
            return 0;
        };
        let remaining = data.len().saturating_sub(*cursor);
        let n = remaining.min(buffer.len());
        buffer[..n].copy_from_slice(&data[*cursor..*cursor + n]);
        *cursor += n;
        n as u64
    })
}

/// Finalizes (write handle) or closes (read handle) a blob handle.
///
/// Returns the 32-byte content id, or `None` if the descriptor is unknown.
pub(crate) fn blob_close(fd: u64) -> Option<[u8; 32]> {
    with(|h| {
        if let Some(buf) = h.blob_write_handles.remove(&fd) {
            let mut hasher = Sha256::new();
            hasher.update(&buf);
            let blob_id: [u8; 32] = hasher.finalize().into();
            h.blobs.insert(blob_id, buf);
            return Some(blob_id);
        }
        if let Some((blob_id, _, _)) = h.blob_read_handles.remove(&fd) {
            return Some(blob_id);
        }
        None
    })
}

/// Returns a strictly-increasing nanosecond timestamp.
///
/// Seeded from the wall clock but bumped to at least `last + 1` so two reads in
/// the same nanosecond never collide — otherwise time-ordered logic (e.g. two
/// `LwwRegister` writes) could tie on a fast machine and silently keep the wrong
/// side.
pub(crate) fn time_now() -> u64 {
    with(|h| {
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        h.last_time = h.last_time.max(wall).saturating_add(1);
        h.last_time
    })
}

/// Fills `buf` with pseudo-random bytes from a thread-local splitmix64 PRNG.
///
/// Tests don't need cryptographic randomness — only that `random_bytes`
/// doesn't trap and that repeated calls differ — so this avoids pulling a
/// crypto RNG dependency into the SDK.
pub(crate) fn random_bytes(buf: &mut [u8]) {
    with(|h| {
        for chunk in buf.chunks_mut(8) {
            // splitmix64
            h.rng_state = h.rng_state.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = h.rng_state;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let bytes = z.to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    });
}
