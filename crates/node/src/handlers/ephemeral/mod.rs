//! Ephemeral presence channel — in-memory awareness subsystem.
//!
//! **No network, no storage, no actix imports in this module tree.**
//! This is the structural "never persisted" boundary for the presence feature.

/// Milliseconds between local heartbeat re-publishes (sender side).
pub const PRESENCE_HEARTBEAT_MS: u64 = 2_500;

/// Milliseconds before an entry with no heartbeat is considered stale and
/// swept from the in-memory store.
pub const PRESENCE_TTL_MS: u64 = 7_000;

/// Maximum byte length of a single ephemeral awareness slice.
///
/// Re-exported from `calimero-primitives` — the single source of truth shared
/// with the JSON-RPC `set_ephemeral` handler in `calimero-server` — so the
/// node's enforcement and the server's pre-validation can never drift.
pub use calimero_primitives::events::EPHEMERAL_MAX_BYTES;

pub mod auth;
pub(crate) mod inbound;
pub(crate) mod outbound;
pub mod store;
