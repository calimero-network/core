use std::sync::Arc;

use calimero_node_primitives::SyncPhase;
use calimero_server_primitives::jsonrpc::{
    SyncState, SyncStatusError, SyncStatusRequest, SyncStatusResponse,
};
use tracing::debug;

use super::{Request, RpcError, ServiceState};

impl Request for SyncStatusRequest {
    type Response = SyncStatusResponse;
    type Error = SyncStatusError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        let context_id = self.context_id;

        // `is_initialized` is the authoritative "is the state here yet?" fact,
        // read from the context's root hash. An all-zero root hash is the same
        // condition that makes `execute` return `Uninitialized`. A missing
        // context is an expected client error — log at debug so a caller can't
        // flood error-level logs by probing arbitrary ids.
        let Some(context) = state.ctx_client.get_context(&context_id)? else {
            debug!(%context_id, "sync_status requested for unknown context");
            return Err(RpcError::MethodCallError(SyncStatusError::ContextNotFound));
        };
        let is_initialized = *context.root_hash != [0; 32];

        // The run-loop's advisory snapshot supplies the phase. The node's
        // `SyncPhase` is blind to initialization, so resolve the ambiguous
        // settled-but-no-state case here: an uninitialized context that the
        // run-loop considers idle (including the benign no-peers / not-
        // materialised outcome, which clears the in-flight marker without
        // recording a failure, and the never-dispatched case) is waiting for a
        // peer to sync from. `Syncing` / `BackingOff` are reported as-is
        // regardless of initialization — an initialized context can still be
        // catching up on later deltas.
        let snapshot = state.node_client.sync_status(context_id).await?;

        let sync_state = resolve_sync_state(snapshot.as_ref().map(|s| s.phase), is_initialized);
        let (failure_count, last_error) = snapshot
            .map(|s| (s.failure_count, s.last_error))
            .unwrap_or((0, None));

        Ok(SyncStatusResponse::new(
            context_id,
            is_initialized,
            sync_state,
            failure_count,
            last_error,
        ))
    }
}

/// Resolve the wire-facing [`SyncState`] from the run-loop's advisory phase and
/// the authoritative `is_initialized` flag.
///
/// `Syncing` and `BackingOff` are reported as-is — an initialized context can
/// legitimately still be catching up on later deltas. The ambiguity is the
/// settled-but-idle case: the node's `SyncPhase::Idle` (and the never-dispatched
/// `None`) covers both a healthy initialized context *and* one that has no state
/// yet and is simply waiting for a peer — including the benign no-peers /
/// peer-not-materialised outcome, which clears the in-flight marker without
/// recording a failure. `is_initialized` disambiguates the two.
fn resolve_sync_state(phase: Option<SyncPhase>, is_initialized: bool) -> SyncState {
    match phase {
        Some(SyncPhase::Syncing) => SyncState::Syncing,
        Some(SyncPhase::BackingOff { retry_in_secs }) => SyncState::BackingOff { retry_in_secs },
        Some(SyncPhase::Idle) | None if is_initialized => SyncState::Idle,
        Some(SyncPhase::Idle) | None => SyncState::WaitingForPeers,
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_sync_state, SyncPhase, SyncState};

    #[test]
    fn uninitialized_and_idle_resolves_to_waiting_for_peers() {
        // The bug this guards: a benign no-peers outcome leaves the node phase
        // at Idle; without initialization context that would look "settled".
        assert!(matches!(
            resolve_sync_state(Some(SyncPhase::Idle), false),
            SyncState::WaitingForPeers
        ));
    }

    #[test]
    fn uninitialized_and_never_dispatched_resolves_to_waiting_for_peers() {
        assert!(matches!(
            resolve_sync_state(None, false),
            SyncState::WaitingForPeers
        ));
    }

    #[test]
    fn initialized_and_idle_resolves_to_idle() {
        assert!(matches!(
            resolve_sync_state(Some(SyncPhase::Idle), true),
            SyncState::Idle
        ));
        assert!(matches!(resolve_sync_state(None, true), SyncState::Idle));
    }

    #[test]
    fn syncing_and_backing_off_are_reported_regardless_of_initialization() {
        for is_init in [false, true] {
            assert!(matches!(
                resolve_sync_state(Some(SyncPhase::Syncing), is_init),
                SyncState::Syncing
            ));
            assert!(matches!(
                resolve_sync_state(Some(SyncPhase::BackingOff { retry_in_secs: 8 }), is_init),
                SyncState::BackingOff { retry_in_secs: 8 }
            ));
        }
    }
}
