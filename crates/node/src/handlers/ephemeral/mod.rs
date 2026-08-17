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

/// Milliseconds since the UNIX epoch — the single wall-clock reading the whole
/// presence subsystem uses (inbound apply, outbound publish and heartbeat
/// sweep, and the snapshot RPC), so a `now_ms` can never mean two different
/// things across the three.
///
/// # Pre-epoch clock
///
/// `duration_since(UNIX_EPOCH)` fails only when the host clock is set before
/// 1970. Rather than panic, this degrades to `0`, and the honest description of
/// what that produces is **"presence freezes"**, not "everything looks
/// maximally aged":
///
/// - ages are computed as `now_ms.saturating_sub(last_seen_ms)`, so with
///   `now_ms == 0` every entry reports `age_ms == 0` — maximally *fresh*;
/// - the TTL sweep tests `now_ms.saturating_sub(last_seen_ms) >= ttl_ms`, which
///   is therefore never true, so nothing expires for as long as the clock reads
///   pre-epoch.
///
/// That is deliberately preferred to the "conservative" alternative of
/// returning [`u64::MAX`], which is worse in the case that actually matters —
/// clock *recovery*. Entries stamped `u64::MAX` would, once the clock is sane
/// again, report `now_ms.saturating_sub(u64::MAX) == 0` forever: permanently
/// fresh, never swept, a genuine leak. Stamping `0` is self-healing in exactly
/// the same situation — a real `now_ms` minus `0` exceeds any TTL, so every
/// entry written during the outage is swept on the first tick after recovery.
///
/// The exposure while the clock is broken is bounded: entries stop expiring,
/// but [`store::MAX_AUTHORS_PER_CONTEXT`] still bounds how many can accumulate,
/// nothing is persisted, and presence is a best-effort signal that clients are
/// already told not to treat as authoritative.
pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub mod auth;
pub(crate) mod inbound;
pub(crate) mod outbound;
pub mod store;
