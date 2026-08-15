//! Ephemeral presence channel — in-memory awareness subsystem.
//!
//! **No network, no storage, no actix imports in this module tree.**
//! This is the structural "never persisted" boundary for the presence feature.

/// Milliseconds between local heartbeat re-publishes (sender side).
pub const PRESENCE_HEARTBEAT_MS: u64 = 2_500;

/// Milliseconds before an entry with no heartbeat is considered stale and
/// swept from the in-memory store.
pub const PRESENCE_TTL_MS: u64 = 7_000;

/// How far a received envelope's signed `sent_at_ms` may sit from the
/// receiver's own clock, in either direction, before it is dropped as a replay.
///
/// **This is a deliberate trade, and 30s is the chosen point on it.**
/// `sent_at_ms` is stamped from the *sender's* wall clock, so the window has to
/// absorb the clock skew between two independent machines. Too tight and two
/// nodes a few seconds apart reject each other's presence outright — the
/// feature simply stops working, with a silent `debug!` as the only trace. Too
/// loose and a recorded envelope stays replayable for that whole span. 30s
/// tolerates the skew real machines exhibit (NTP-synced hosts sit well inside
/// it; even an un-synced host is usually within seconds) while bounding replay
/// to a fixed window instead of the unbounded one a receiver with no freshness
/// binding offers.
///
/// Note the window is not the whole story: a replay inside it still has to beat
/// the LWW `seq` rule in [`store::AwarenessStore::apply`], so it can only
/// resurrect an author whose entry has already TTL-swept.
pub const PRESENCE_MAX_SKEW_MS: u64 = 30_000;

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
