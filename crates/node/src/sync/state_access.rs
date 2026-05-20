//! Trait abstraction for the subset of `NodeState` that `sync/` reads
//! and mutates.
//!
//! Why this exists: `sync/` used to reach directly into `NodeState`'s
//! fields (`delta_stores`, `peer_identities`, `reconcile_attempts`)
//! and call its methods (`end_sync_session`, `cancel_sync_session`).
//! That coupling made the sync module impossible to unit-test without
//! standing up a full `NodeManager` — every interesting failure mode
//! had to be engineered as an integration test against the real actor
//! stack. The trait inverts the dependency: `sync/` knows only this
//! interface, and `NodeState` implements it. Tests can substitute a
//! recording fake (`MockSyncStateAccess`) and exercise sync paths in
//! isolation.
//!
//! Scope: data access only. Behavioural concerns that already have a
//! trait (network in [`crate::sync::network::SyncNetwork`], storage in
//! [`calimero_storage::store`]) stay in their own traits. Cross-actor
//! function calls (the lone `crate::handlers::state_delta::replay_buffered_delta`
//! call site) are out of scope here — pass a closure to that call site
//! or convert it to an actor message in a follow-up.

use std::collections::BTreeSet;
use std::time::Duration;

use calimero_node_primitives::delta_buffer::BufferedDelta;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;

use crate::delta_store::DeltaStore;

/// Access surface from `sync/` into `NodeState`.
///
/// The methods here are the entire set of `NodeState` touchpoints
/// the sync module needs. Every call to `self.node_state.<field>` or
/// `self.node_state.<method>` inside `sync/` goes through this
/// trait; that's enforced by `sync/` only depending on `&dyn
/// SyncStateAccess`, never on the concrete `NodeState`.
pub(crate) trait SyncStateAccess: Send + Sync {
    /// Look up the [`DeltaStore`] registered for `context_id`.
    ///
    /// Returns `None` if no sync has been initiated for that context
    /// yet — the store is lazily created on first sync.
    fn delta_store(&self, context_id: &ContextId) -> Option<DeltaStore>;

    /// Register a freshly-constructed [`DeltaStore`] for `context_id`.
    ///
    /// Overwrites any prior registration. Called from sync paths that
    /// realise a context is new (first interval sync, first reconcile,
    /// first DAG catchup).
    fn register_delta_store(&self, context_id: ContextId, store: DeltaStore);

    /// End the active sync session for `context_id` and return any
    /// deltas the session buffered.
    ///
    /// Returns `None` if no session was active (caller already handled
    /// session end or it was never started). See
    /// `NodeState::end_sync_session` for the buffer-drop diagnostics
    /// the production impl emits on session end with leftover deltas.
    fn end_sync_session(&self, context_id: &ContextId) -> Option<Vec<BufferedDelta>>;

    /// Cancel the active sync session for `context_id` and discard
    /// buffered deltas without surfacing them. Called on sync failure
    /// where the in-flight deltas can't be replayed safely.
    fn cancel_sync_session(&self, context_id: &ContextId);

    /// Identities `peer_id` has been observed signing applied messages
    /// with.
    ///
    /// Returns `None` if the peer has never been observed. The
    /// returned set is a clone — callers iterate it without holding
    /// the underlying `DashMap` shard lock.
    fn peer_identities(&self, peer_id: &PeerId) -> Option<BTreeSet<PublicKey>>;

    /// Remaining cooldown for the reconcile-after-divergence path on
    /// `context_id`, plus the current `consecutive_failures` count.
    ///
    /// Returns `None` if either no failure has been recorded or the
    /// cooldown has elapsed — both interpreted as "reconcile is
    /// allowed right now."
    fn reconcile_remaining_cooldown(&self, context_id: &ContextId) -> Option<(Duration, u32)>;

    /// Clear backoff state for `context_id` after a successful
    /// reconcile. The next divergence is treated as a fresh attempt.
    fn record_reconcile_success(&self, context_id: &ContextId);

    /// Record a reconcile failure for `context_id`: bump
    /// `consecutive_failures`, stamp `last_attempt_at = now`. Returns
    /// the new failure count so the caller can compute the next
    /// cooldown directly.
    fn record_reconcile_failure(&self, context_id: ContextId) -> u32;
}
