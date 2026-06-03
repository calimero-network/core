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
//! lock-free map and read out-of-band (or pushed as a WebSocket event), so a
//! reader may observe a value that is a few hundred milliseconds stale. It is
//! meant for UX ("syncing, please wait" vs "stuck"), not for control flow.
//!
//! The phase vocabulary ([`SyncState`]) is the shared wire type from
//! `calimero-primitives`, so the run-loop, the JSON-RPC response, and the
//! WebSocket event all speak the same language.

pub use calimero_primitives::sync_status::SyncState;

/// A snapshot of where a context is in the sync lifecycle.
#[derive(Clone, Debug)]
pub struct SyncStatusSnapshot {
    /// Coarse phase the sync run-loop last recorded for this context.
    pub state: SyncState,
    /// Consecutive failed sync attempts. Resets to 0 on the next success.
    /// A non-zero value with `state == BackingOff` is the "stuck" signal.
    pub failure_count: u32,
    /// The error from the most recent failed attempt, if any. Carries the
    /// reason behind a `BackingOff` state (e.g. "No peers to sync with").
    pub last_error: Option<String>,
}
