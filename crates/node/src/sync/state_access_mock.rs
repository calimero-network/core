//! Scriptable mock for [`super::state_access::SyncStateAccess`].
//!
//! Records every trait-method call and lets tests inject specific
//! return values per-context or per-peer. Drop-in for `Arc<dyn
//! SyncStateAccess>` in sync's `SyncManager.state_access` field, so
//! tests can drive `SyncManager` paths without standing up a full
//! `NodeManager` + `NodeState`.
//!
//! Counterpart to [`crate::sync::network::mock::MockSyncNetwork`] —
//! same `parking_lot::Mutex` choice (never poisons on panic), same
//! drop-in pattern. Together they cover the two non-storage,
//! non-protocol surfaces a `SyncManager` reaches into.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::Duration;

use calimero_node_primitives::delta_buffer::BufferedDelta;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use libp2p::PeerId;
use parking_lot::Mutex;

use super::state_access::SyncStateAccess;
use crate::delta_store::DeltaStore;
use crate::sync::reconcile_cooldown;

/// Record of a single trait-method call. Useful for asserting
/// what the code under test asked for, in what order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncStateAccessCall {
    DeltaStore(ContextId),
    GetOrRegisterDeltaStore {
        context_id: ContextId,
        created: bool,
    },
    EndSyncSession(ContextId),
    CancelSyncSession(ContextId),
    PeerIdentities(PeerId),
    ReconcileRemainingCooldown(ContextId),
    RecordReconcileSuccess(ContextId),
    RecordReconcileFailure(ContextId),
}

/// Scriptable `SyncStateAccess` fixture. All getters return `None`
/// (or `0` for the reconcile-failure counter) by default; tests
/// override per-key behaviour via the `set_*` / `push_*` helpers
/// below.
#[derive(Default)]
pub(crate) struct MockSyncStateAccess {
    delta_stores: Mutex<HashMap<ContextId, DeltaStore>>,
    peer_identities: Mutex<HashMap<PeerId, BTreeSet<PublicKey>>>,
    end_sync_session_responses: Mutex<HashMap<ContextId, VecDeque<Option<Vec<BufferedDelta>>>>>,
    reconcile_cooldowns: Mutex<HashMap<ContextId, (Duration, u32)>>,
    failure_counts: Mutex<HashMap<ContextId, u32>>,
    calls: Mutex<Vec<SyncStateAccessCall>>,
}

impl MockSyncStateAccess {
    /// Register a `DeltaStore` so subsequent `delta_store(ctx)` calls
    /// return it (rather than `None`) and `get_or_register_delta_store`
    /// reports `created=false`.
    pub(crate) fn insert_delta_store(&self, context_id: ContextId, store: DeltaStore) {
        let _replaced = self.delta_stores.lock().insert(context_id, store);
    }

    /// Inject a `peer_identities` response for `peer`.
    pub(crate) fn insert_peer_identities(&self, peer: PeerId, ids: BTreeSet<PublicKey>) {
        let _replaced = self.peer_identities.lock().insert(peer, ids);
    }

    /// Queue a response for the next `end_sync_session(ctx)` call.
    /// Returns are popped FIFO per `context_id`; once exhausted the
    /// method returns `None`. **Note**: a queued `None` and the
    /// exhausted-queue case both return `None` from `end_sync_session`
    /// — tests that need to distinguish the two should also assert
    /// on `calls()` length, not just the return value.
    pub(crate) fn push_end_sync_session_response(
        &self,
        context_id: ContextId,
        response: Option<Vec<BufferedDelta>>,
    ) {
        self.end_sync_session_responses
            .lock()
            .entry(context_id)
            .or_default()
            .push_back(response);
    }

    /// Pre-set the cooldown that `reconcile_remaining_cooldown(ctx)`
    /// returns. The mock does not advance time; the test owns the
    /// observed value end-to-end. Useful when a test wants to start
    /// in a pre-existing-cooldown state without driving N failures
    /// to get there; otherwise prefer calling `record_reconcile_failure`
    /// directly — that path now installs a cooldown the same way the
    /// production impl does (see [`SyncStateAccess::record_reconcile_failure`]'s
    /// mock body).
    pub(crate) fn set_reconcile_cooldown(
        &self,
        context_id: ContextId,
        cooldown: Duration,
        consecutive_failures: u32,
    ) {
        let _replaced = self
            .reconcile_cooldowns
            .lock()
            .insert(context_id, (cooldown, consecutive_failures));
    }

    /// Snapshot of every trait-method call observed so far, in order.
    /// Tests can assert on this directly.
    pub(crate) fn calls(&self) -> Vec<SyncStateAccessCall> {
        self.calls.lock().clone()
    }
}

impl SyncStateAccess for MockSyncStateAccess {
    fn delta_store(&self, context_id: &ContextId) -> Option<DeltaStore> {
        self.calls
            .lock()
            .push(SyncStateAccessCall::DeltaStore(*context_id));
        self.delta_stores.lock().get(context_id).cloned()
    }

    fn get_or_register_delta_store(
        &self,
        context_id: ContextId,
        factory: Box<dyn FnOnce() -> DeltaStore + Send>,
    ) -> (DeltaStore, bool) {
        // Record the call *before* releasing the stores lock so a
        // concurrent reader of `calls()` can never observe an inserted
        // store without the matching log entry. Matches the call-then-
        // return ordering of every other trait method in this impl.
        let mut stores = self.delta_stores.lock();
        match stores.get(&context_id) {
            Some(existing) => {
                let store = existing.clone();
                self.calls
                    .lock()
                    .push(SyncStateAccessCall::GetOrRegisterDeltaStore {
                        context_id,
                        created: false,
                    });
                drop(stores);
                (store, false)
            }
            None => {
                let store = factory();
                let _replaced = stores.insert(context_id, store.clone());
                self.calls
                    .lock()
                    .push(SyncStateAccessCall::GetOrRegisterDeltaStore {
                        context_id,
                        created: true,
                    });
                drop(stores);
                (store, true)
            }
        }
    }

    fn end_sync_session(&self, context_id: &ContextId) -> Option<Vec<BufferedDelta>> {
        self.calls
            .lock()
            .push(SyncStateAccessCall::EndSyncSession(*context_id));
        self.end_sync_session_responses
            .lock()
            .get_mut(context_id)
            .and_then(|q| q.pop_front())
            .unwrap_or(None)
    }

    fn cancel_sync_session(&self, context_id: &ContextId) {
        self.calls
            .lock()
            .push(SyncStateAccessCall::CancelSyncSession(*context_id));
    }

    fn peer_identities(&self, peer_id: &PeerId) -> Option<BTreeSet<PublicKey>> {
        self.calls
            .lock()
            .push(SyncStateAccessCall::PeerIdentities(*peer_id));
        self.peer_identities.lock().get(peer_id).cloned()
    }

    fn reconcile_remaining_cooldown(&self, context_id: &ContextId) -> Option<(Duration, u32)> {
        self.calls
            .lock()
            .push(SyncStateAccessCall::ReconcileRemainingCooldown(*context_id));
        self.reconcile_cooldowns.lock().get(context_id).copied()
    }

    fn record_reconcile_success(&self, context_id: &ContextId) {
        self.calls
            .lock()
            .push(SyncStateAccessCall::RecordReconcileSuccess(*context_id));
        let _ = self.reconcile_cooldowns.lock().remove(context_id);
        let _ = self.failure_counts.lock().remove(context_id);
    }

    fn record_reconcile_failure(&self, context_id: ContextId) -> u32 {
        self.calls
            .lock()
            .push(SyncStateAccessCall::RecordReconcileFailure(context_id));
        let mut counts = self.failure_counts.lock();
        let entry = counts.entry(context_id).or_insert(0);
        *entry = entry.saturating_add(1);
        let failures = *entry;
        drop(counts);
        // Mirror production: a failure also installs a cooldown
        // computed from the new `consecutive_failures`. Without this,
        // `reconcile_remaining_cooldown` would return `None` after a
        // failure (because the cooldowns map is only populated via
        // explicit `set_reconcile_cooldown`), which diverges from the
        // real `NodeState` impl where one DashMap holds both fields
        // of `ReconcileAttempt` together.
        let cooldown = reconcile_cooldown(failures);
        let _replaced = self
            .reconcile_cooldowns
            .lock()
            .insert(context_id, (cooldown, failures));
        failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(byte: u8) -> ContextId {
        ContextId::from([byte; 32])
    }

    #[test]
    fn defaults_return_none_or_zero_and_calls_are_recorded() {
        let mock = MockSyncStateAccess::default();
        assert!(mock.delta_store(&ctx(1)).is_none());
        assert!(mock.end_sync_session(&ctx(1)).is_none());
        assert!(mock.peer_identities(&PeerId::random()).is_none());
        // Cooldown is None until a failure (or explicit
        // `set_reconcile_cooldown`) installs one.
        assert!(mock.reconcile_remaining_cooldown(&ctx(2)).is_none());
        assert_eq!(mock.record_reconcile_failure(ctx(2)), 1);
        // The first failure installs a cooldown — `reconcile_remaining_cooldown`
        // returns Some, mirroring production behaviour.
        assert!(mock.reconcile_remaining_cooldown(&ctx(2)).is_some());
        assert_eq!(mock.record_reconcile_failure(ctx(2)), 2);
        assert_eq!(mock.record_reconcile_failure(ctx(3)), 1);
        // record_reconcile_success clears the count and the cooldown.
        mock.record_reconcile_success(&ctx(2));
        assert!(mock.reconcile_remaining_cooldown(&ctx(2)).is_none());
        assert_eq!(mock.record_reconcile_failure(ctx(2)), 1);

        // `calls()` records every trait-method invocation in order;
        // tests can pattern-match the full sequence to assert
        // ordering. Sanity-check the first entry; assertion shapes
        // for specific sequences belong in higher-level tests.
        let calls = mock.calls();
        assert!(matches!(
            calls.first(),
            Some(SyncStateAccessCall::DeltaStore(_))
        ));
    }

    #[test]
    fn end_sync_session_responses_pop_per_context_fifo() {
        let mock = MockSyncStateAccess::default();
        mock.push_end_sync_session_response(ctx(1), Some(vec![]));
        mock.push_end_sync_session_response(ctx(1), None);
        // First call returns the queued Some.
        assert!(mock.end_sync_session(&ctx(1)).is_some());
        // Second call returns the queued None.
        assert!(mock.end_sync_session(&ctx(1)).is_none());
        // Third call (exhausted) returns None by default.
        assert!(mock.end_sync_session(&ctx(1)).is_none());
    }
}
