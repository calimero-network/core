//! Freshness for presence.
//!
//! Authorship is the author's signature on a `PresenceStatement`
//! (`calimero_node_primitives::presence`), which also covers the
//! `sent_at_ms` this module judges. The statement travels inside the AEAD,
//! so it cannot be restamped in flight.
//!
//! # Security — freshness is bound into the signature
//!
//! The signed payload carries the sender's `sent_at_ms` wall clock, and the
//! receive path refuses an envelope whose stamp is further than
//! [`PRESENCE_MAX_SKEW_MS`] from the receiver's own clock in either direction.
//!
//! This closes a replay hole that authorship alone could not: any gossipsub
//! mesh peer subscribed to a context's presence topic — which requires no
//! group key, only mesh membership — can record one valid envelope from
//! `author` at some `seq`. Without a freshness binding, once `author` goes
//! idle and the entry TTL-sweeps on receivers
//! ([`crate::handlers::ephemeral::PRESENCE_TTL_MS`], 7s), the recorder could
//! re-inject the exact same bytes forever: the signature still verifies, the
//! key can still be current, and with no local entry left `seq` looks fresh
//! again — leaving a departed peer rendered present indefinitely.
//!
//! `sent_at_ms` must live **inside** the signed payload. Carried alongside it,
//! the recorder would simply restamp it, since the AEAD is sealed with empty
//! associated data and the field rides in the clear.
//!
//! [`PRESENCE_MAX_SKEW_MS`] is set equal to
//! [`PRESENCE_TTL_MS`](crate::handlers::ephemeral::PRESENCE_TTL_MS) so the
//! window closes at essentially the moment a TTL sweep would make a recorded
//! envelope useful. Writing the two conditions out against a common timeline —
//! `T` the sender's publish instant, `d >= 0` the delivery delay, `s` how far
//! the sender's clock runs *ahead* of the receiver's:
//!
//! * the receiver stamps `last_seen` at `T + d`, so the entry sweeps at
//!   `T + d + PRESENCE_TTL_MS`, which is when a replay could first take effect
//!   (before that, the LWW `seq` rule makes it a no-op);
//! * `is_fresh` accepts until `T + s + PRESENCE_MAX_SKEW_MS`.
//!
//! The replay is therefore effective only on `[T + d + TTL, T + s + SKEW]`,
//! which with `SKEW == TTL` is empty unless `s > d` — the sender's clock must
//! run ahead of the receiver's by more than the delivery delay, and the
//! surviving window is exactly that excess, `s - d`. It is bounded by real
//! clock skew (single-digit milliseconds between NTP-synced hosts), not by the
//! TTL, and it is not attacker-amplifiable: `s` belongs to the honest sender,
//! and `sent_at_ms` is inside the signature so a recorder cannot restamp it.
//!
//! Closing the residual entirely would take a per-author tombstone surviving
//! the sweep (rejecting a `sent_at_ms` at or before the last swept entry's).
//! That is deliberately not done: it reintroduces exactly the unbounded
//! per-author growth the store is careful to avoid, needing its own cap and
//! expiry, to buy back a few milliseconds. See [`PRESENCE_MAX_SKEW_MS`] for the
//! skew-vs-replay trade this sits on.
//!
//! No I/O, no actix, no store access.

use crate::handlers::ephemeral::PRESENCE_MAX_SKEW_MS;

/// Is `sent_at_ms` close enough to `now_ms` to accept?
///
/// The window is symmetric: a far-future stamp is as suspicious as a stale one
/// (and, left unchecked, a stamp far enough ahead would stay "fresh" for as
/// long as the attacker cared to keep replaying it).
///
/// See [`PRESENCE_MAX_SKEW_MS`] for why the window is sized the way it is.
pub fn is_fresh(now_ms: u64, sent_at_ms: u64) -> bool {
    now_ms.abs_diff(sent_at_ms) <= PRESENCE_MAX_SKEW_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_window_is_symmetric_and_bounded() {
        let now = 1_000_000_000_000u64;
        assert!(is_fresh(now, now), "an unskewed stamp is fresh");
        assert!(
            is_fresh(now, now - PRESENCE_MAX_SKEW_MS),
            "a stamp exactly at the stale edge is still accepted"
        );
        assert!(
            is_fresh(now, now + PRESENCE_MAX_SKEW_MS),
            "a stamp exactly at the future edge is still accepted"
        );
        assert!(
            !is_fresh(now, now - PRESENCE_MAX_SKEW_MS - 1),
            "a stamp past the stale edge is refused"
        );
        assert!(
            !is_fresh(now, now + PRESENCE_MAX_SKEW_MS + 1),
            "a far-future stamp is refused too — the window is symmetric"
        );
    }
}
