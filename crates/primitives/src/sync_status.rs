//! Wire-facing sync-status types shared by the `sync_status` JSON-RPC response
//! and the `SyncStatus` WebSocket event, so both speak one vocabulary.
//!
//! See `calimero-node`'s sync run-loop for where these are produced, and the
//! issue this serves: a node blocked on `Uninitialized` needs to tell
//! "syncing" from "waiting for a peer" from "stuck".

use serde::{Deserialize, Serialize};

/// Coarse sync phase. Serialized internally-tagged as `{ "state": "syncing" }`,
/// with data-carrying variants adding their fields alongside the tag (e.g.
/// `{ "state": "backingOff", "retryInSecs": 8 }`). A typed enum keeps each
/// variant's data bound to it and lets clients match exhaustively.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum SyncState {
    /// Settled — no sync in flight, nothing pending.
    Idle,
    /// Not yet initialized and nothing is actively syncing: typically waiting
    /// for a co-member peer to appear to sync the initial state from.
    WaitingForPeers,
    /// A sync attempt is in flight (handshake / delta exchange).
    Syncing,
    /// Receiving an initial-state snapshot. `records_received` is a monotonic
    /// count of applied entries — a liveness/progress signal. It is not a
    /// percentage: the snapshot stream is cursor-paged and the receiver is not
    /// told a grand total up front, so a true percent/ETA would require the
    /// sender to advertise one (a follow-up wire change).
    ReceivingSnapshot {
        /// Entries applied from the snapshot so far.
        records_received: u64,
    },
    /// The last attempt failed and the next retry is gated behind backoff.
    BackingOff {
        /// Estimated seconds until the next retry is eligible.
        retry_in_secs: u64,
    },
}
