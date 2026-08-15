//! Replay-on-subscribe for ephemeral presence.
//!
//! Presence changes ride the node's event broadcast as
//! [`ContextEventPayload::Ephemeral`] deltas, which every transport already
//! fans out to its authorized subscribers. That covers *changes* — but a
//! client that connects into an already-populated context would see nothing
//! until each peer's slice next changed (heartbeats that re-send identical
//! bytes produce no diff, by design), so it needs to be seeded with the
//! context's current entries.
//!
//! This module builds that seed as ordinary `Ephemeral` events, so seeding is
//! the *same* read path as the live stream — one wire shape, one authorization
//! gate — instead of a second endpoint with its own of each. The only
//! difference on the wire is [`EphemeralPayload::age_ms`], which is present on
//! a replayed entry and absent on a live delta.
//!
//! # Delivery is per-connection
//!
//! The events built here MUST be pushed into the subscribing connection's own
//! sink, never into the node-wide event broadcast: every subscriber drains
//! that broadcast, so replaying through it would re-deliver one client's seed
//! to every other already-connected client, every time anyone subscribed. Each
//! transport therefore owns the delivery half (`ConnectionState::try_push_event`
//! for WS, `SessionState::try_push_event` for SSE) and this module only builds
//! the payloads.
//!
//! # Ordering
//!
//! Callers must subscribe FIRST and replay SECOND. Reading the snapshot before
//! the subscription goes live would drop any delta landing in between — it
//! would be in neither the (already-read) snapshot nor the (not-yet-live)
//! stream. Subscribing first can only duplicate: a delta delivered between the
//! subscription going live and the replay lands ahead of a replayed entry that
//! is at least as new, because the awareness store is written *before* the
//! diff is emitted (`calimero-node`'s `handlers::ephemeral::inbound`).
//!
//! # One timeout for the whole subscribe, not one per context
//!
//! A subscribe naming N contexts asks the node actor for N snapshots. Awaited
//! one after another, each context's [`SNAPSHOT_TIMEOUT`] stacks: a degraded
//! actor turns an M-context subscribe into an `M * SNAPSHOT_TIMEOUT` stall
//! before the client gets *any* acknowledgment — and the ack is what tells it
//! the live stream is up. [`presence_replay_many`] drives all N concurrently,
//! so the worst case is one timeout regardless of how many contexts the client
//! subscribed to. Callers with more than one context must use it rather than
//! looping over [`presence_replay`].

use std::time::Duration;

use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::events::{ContextEvent, ContextEventPayload, EphemeralPayload, NodeEvent};
use tracing::debug;

/// How long to wait for the node actor to answer a presence-snapshot read
/// before giving up on seeding the subscriber.
///
/// The read is an in-memory map lookup on the actor thread, so this bound is
/// never reached in a healthy node. It exists because the subscribe handler
/// awaits it inline: without a bound, a saturated or not-yet-started node actor
/// would stall the subscription itself — the client would get no acknowledgment
/// and, worse, no live stream either, over an optional seed. Anything slower
/// than this is also worthless as a seed: presence republishes every
/// `PRESENCE_HEARTBEAT_MS` (2.5s), so a client that skips the seed is no more
/// than one heartbeat behind.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

/// Build the replay events seeding a new subscriber with `context_id`'s
/// current presence.
///
/// Returns an empty `Vec` when the context has no live entries, and also when
/// the snapshot read fails or times out ([`SNAPSHOT_TIMEOUT`]): presence is
/// best-effort transient state, so a failed seed degrades to "you start empty
/// and converge on the next delta" (at most one heartbeat,
/// `PRESENCE_HEARTBEAT_MS`) rather than failing the subscription — the same
/// silent-drop contract the receive path uses.
///
/// Every entry is an upsert (`removed: false`): the snapshot only contains
/// entries that are live, and a removal is not a thing a new subscriber needs
/// to be told about — it has no prior state to remove.
pub(crate) async fn presence_replay(
    node_client: &NodeClient,
    context_id: ContextId,
) -> Vec<NodeEvent> {
    let snapshot =
        tokio::time::timeout(SNAPSHOT_TIMEOUT, node_client.ephemeral_snapshot(context_id)).await;

    let entries = match snapshot {
        Ok(Ok(entries)) => entries,
        Err(_elapsed) => {
            debug!(
                %context_id,
                "ephemeral: presence snapshot timed out; subscriber starts unseeded"
            );
            return Vec::new();
        }
        Ok(Err(err)) => {
            debug!(%context_id, %err, "ephemeral: presence snapshot unavailable; subscriber starts unseeded");
            return Vec::new();
        }
    };

    entries
        .into_iter()
        .map(|(author, state, age_ms)| {
            NodeEvent::Context(ContextEvent {
                context_id,
                payload: ContextEventPayload::Ephemeral(EphemeralPayload {
                    author,
                    state: Some(state),
                    removed: false,
                    // The one wire difference from a live delta: a replayed
                    // entry can be up to PRESENCE_TTL_MS stale and the bytes
                    // alone do not say how stale.
                    age_ms: Some(age_ms),
                }),
            })
        })
        .collect()
}

/// Replay events for several contexts at once, paired with the context each
/// batch belongs to (callers need it for logging and, on SSE, for the
/// per-context drop message).
///
/// Every snapshot read is driven **concurrently**, so [`SNAPSHOT_TIMEOUT`]
/// bounds the whole call rather than each context in turn — see the module
/// doc. Order of the returned batches matches `context_ids`; within a batch,
/// order is the snapshot's own (author-sorted).
///
/// Each context still degrades independently: a context whose snapshot fails
/// or times out contributes an empty batch and does not affect the others.
pub(crate) async fn presence_replay_many(
    node_client: &NodeClient,
    context_ids: &[ContextId],
) -> Vec<(ContextId, Vec<NodeEvent>)> {
    futures_util::future::join_all(context_ids.iter().map(|context_id| async move {
        (*context_id, presence_replay(node_client, *context_id).await)
    }))
    .await
}
