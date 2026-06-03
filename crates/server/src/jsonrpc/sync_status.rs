use std::sync::Arc;

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

        // The run-loop publishes the phase directly (including WaitingForPeers
        // for the benign no-peers outcome), so this passes it through. The one
        // adjustment: a context with no recorded snapshot defaults to "waiting"
        // when uninitialized, "idle" once it has state.
        let snapshot = state.node_client.sync_status(context_id).await?;
        let (sync_state, failure_count, last_error) = match snapshot {
            Some(snap) => (
                normalize(snap.state, is_initialized),
                snap.failure_count,
                snap.last_error,
            ),
            None if is_initialized => (SyncState::Idle, 0, None),
            None => (SyncState::WaitingForPeers, 0, None),
        };

        Ok(SyncStatusResponse::new(
            context_id,
            is_initialized,
            sync_state,
            failure_count,
            last_error,
        ))
    }
}

/// Safety net for stale snapshots: an initialized context already has its
/// state, so it can't still be "waiting for peers" to deliver it. Every other
/// phase (including `Syncing`/`ReceivingSnapshot` while catching up on later
/// deltas) is reported verbatim.
fn normalize(state: SyncState, is_initialized: bool) -> SyncState {
    match state {
        SyncState::WaitingForPeers if is_initialized => SyncState::Idle,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize, SyncState};

    #[test]
    fn waiting_for_peers_on_initialized_context_becomes_idle() {
        assert!(matches!(
            normalize(SyncState::WaitingForPeers, true),
            SyncState::Idle
        ));
    }

    #[test]
    fn waiting_for_peers_on_uninitialized_context_is_preserved() {
        assert!(matches!(
            normalize(SyncState::WaitingForPeers, false),
            SyncState::WaitingForPeers
        ));
    }

    #[test]
    fn active_phases_are_reported_verbatim_regardless_of_initialization() {
        for is_init in [false, true] {
            assert!(matches!(
                normalize(SyncState::Syncing, is_init),
                SyncState::Syncing
            ));
            let snapshot = SyncState::ReceivingSnapshot {
                records_received: 7,
                percent: Some(50),
                eta_secs: Some(3),
            };
            assert!(matches!(
                normalize(snapshot, is_init),
                SyncState::ReceivingSnapshot {
                    records_received: 7,
                    ..
                }
            ));
            assert!(matches!(
                normalize(SyncState::BackingOff { retry_in_secs: 8 }, is_init),
                SyncState::BackingOff { retry_in_secs: 8 }
            ));
        }
    }
}
