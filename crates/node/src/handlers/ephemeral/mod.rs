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
/// **Deliberately equal to [`PRESENCE_TTL_MS`], and the equality is the point.**
///
/// `sent_at_ms` is stamped from the *sender's* wall clock, so this window is
/// spending clock-skew tolerance to buy a bounded replay window, and the two
/// pull in opposite directions. Too tight and two nodes a few seconds apart
/// reject each other's presence outright — the feature stops working with a
/// silent `debug!` as the only trace. Too loose and a recorded envelope stays
/// replayable for that whole span.
///
/// Tying it to the TTL is what makes the replay bound meaningful rather than
/// merely finite. A replay must beat the LWW `seq` rule in
/// [`store::AwarenessStore::apply`] (a re-injected envelope carries the seq it
/// was recorded at, and equal-or-lower seq is a no-op), so it can only take
/// effect on a receiver whose entry for that author has already TTL-swept —
/// i.e. no earlier than `PRESENCE_TTL_MS` after the author's last genuine
/// publish. With the window set to exactly that, the envelope stops being fresh
/// at the same instant the sweep would make it useful: the resurrection window
/// is closed, not merely capped. At the previous 30s the two were 23s apart,
/// which is precisely the interval in which a departed peer could be rendered
/// present again.
///
/// The cost is skew tolerance: 7s each way instead of 30s. That is still two
/// orders of magnitude above what an NTP-synced host exhibits (single-digit
/// milliseconds), and a host drifting more than 7s has clock problems that
/// break far more than presence. Any future widening of this constant must
/// either stay `<= PRESENCE_TTL_MS` or come with a receiver-side seen-envelope
/// cache, since the bound above is the only thing standing between a recorded
/// envelope and a resurrected peer.
pub const PRESENCE_MAX_SKEW_MS: u64 = PRESENCE_TTL_MS;

/// The replay bound above is only real while the freshness window closes no
/// later than the sweep that would make a replay effective. Checked at compile
/// time rather than in a test: it constrains two constants, so a future edit
/// should fail to build, not fail a test run.
const _: () = assert!(
    PRESENCE_MAX_SKEW_MS <= PRESENCE_TTL_MS,
    "a captured presence envelope must stop being fresh no later than the entry it could resurrect expires"
);

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
