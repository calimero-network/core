//! Process-wide signal for `ContextRegistered` governance-op application.
//!
//! When a peer's `join_context` runs before the context→group mapping has
//! propagated via gossipsub, it must wait for the mapping to land locally.
//! Polling `get_group_for_context` after `sync_namespace().await` is racy:
//! `sync_namespace` returns once the sync RPC completes, but the received
//! ops are applied on a separate network-handler task — observed to lag by
//! ~1–2 ms on real networks (merobox CI) and deterministically miss the
//! polling window.
//!
//! This module exposes a broadcast channel that is signalled from the op
//! apply path (`apply_group_op_mutations` → `GroupOp::ContextRegistered`)
//! once the mapping is written. `join_context` subscribes before kicking
//! the sync, then awaits the signal for its target context.
//!
//! Using a module-level `OnceLock` avoids threading the channel handle
//! through `NamespaceGovernance`, `ContextRegistrationService`, and every
//! call site of `apply_group_op_mutations`.

use std::sync::OnceLock;

use calimero_primitives::context::ContextId;
use tokio::sync::broadcast;

/// Broadcast capacity. The channel only carries `ContextId` values and
/// slow subscribers fall back to rechecking the datastore on `Lagged`, so
/// this bound is about bursting multiple near-simultaneous registrations
/// without forcing a lag event in the common case.
const CHANNEL_CAPACITY: usize = 256;

static NOTIFIER: OnceLock<broadcast::Sender<ContextId>> = OnceLock::new();

fn sender() -> &'static broadcast::Sender<ContextId> {
    NOTIFIER.get_or_init(|| broadcast::channel(CHANNEL_CAPACITY).0)
}

/// Signal that a context→group mapping has been written locally. A
/// best-effort broadcast: if there are no subscribers the send is a noop.
pub fn notify(context_id: ContextId) {
    let _ = sender().send(context_id);
}

/// Subscribe to future `ContextRegistered` signals. Subscribe *before*
/// kicking the sync that may deliver the op — otherwise the signal can
/// fire in the gap between sync-apply and subscribe.
pub fn subscribe() -> broadcast::Receiver<ContextId> {
    sender().subscribe()
}
