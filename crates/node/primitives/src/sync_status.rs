//! Best-effort, point-in-time view of a context's state-sync progress.
//!
//! A node that has joined a context but not yet received its initial state
//! reports `root_hash == [0; 32]` and rejects execution with
//! `ExecuteError::Uninitialized`. That single error can't tell a client
//! whether sync is actively running, waiting for a peer to appear, or wedged
//! after repeated failures. This snapshot surfaces the sync manager's own
//! per-context bookkeeping so the distinction is observable.
//!
//! The data is advisory: it is published from the sync run-loop on a
//! lock-free map and read out-of-band, so a reader may observe a value that is
//! a few hundred milliseconds stale. It is meant for UX ("syncing, please
//! wait" vs "stuck"), not for control flow.
//!
//! These types are passed in-process only (run-loop publisher → `NodeManager`
//! → `NodeClient`); the wire-facing shape lives in `calimero-server-primitives`
//! (`jsonrpc::SyncState`), which the server handler maps onto. So there are
//! deliberately no `serde` derives here.

/// A snapshot of where a context is in the sync lifecycle.
#[derive(Clone, Debug)]
pub struct SyncStatusSnapshot {
    /// Coarse phase the sync manager last recorded for this context.
    pub phase: SyncPhase,
    /// Consecutive failed sync attempts. Resets to 0 on the next success.
    /// A non-zero value with `phase == BackingOff` is the "stuck" signal.
    pub failure_count: u32,
    /// The error from the most recent failed attempt, if any. Carries the
    /// reason behind a `BackingOff` phase (e.g. "No peers to sync with"),
    /// which is what lets a client distinguish "waiting for a peer" from a
    /// genuine protocol failure.
    pub last_error: Option<String>,
}

/// Coarse sync phase. Deliberately small: finer-grained snapshot progress
/// (page percent, bytes) can be layered on later without changing the
/// distinction this enum exists to make — running vs settled vs wedged.
///
/// Note this phase is derived purely from the run-loop's `SyncState` and is
/// blind to whether the context is initialized. In particular the benign
/// "no peers / peer not materialised" outcome clears the in-flight marker
/// without recording a failure, so it lands here as [`SyncPhase::Idle`]; it is
/// the *reader* (which also knows `is_initialized`) that resolves an
/// uninitialized-yet-idle context to "waiting for peers".
#[derive(Clone, Copy, Debug)]
pub enum SyncPhase {
    /// No attempt is in flight and none is gated behind backoff.
    Idle,
    /// A sync attempt is currently in flight (snapshot or delta exchange).
    Syncing,
    /// The last attempt finished without success and the next retry is gated
    /// behind exponential backoff. `retry_in_secs` is the manager's best
    /// estimate of the remaining wait; it is `0` once a retry is due.
    BackingOff {
        /// Estimated seconds until the next retry is eligible.
        retry_in_secs: u64,
    },
}
