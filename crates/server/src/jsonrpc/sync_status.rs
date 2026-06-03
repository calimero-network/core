use std::sync::Arc;

use calimero_node_primitives::SyncPhase;
use calimero_server_primitives::jsonrpc::{SyncStatusError, SyncStatusRequest, SyncStatusResponse};
use tracing::error;

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
        // context is a client error, not an internal one.
        let Some(context) = state.ctx_client.get_context(&context_id)? else {
            error!(%context_id, "sync_status requested for unknown context");
            return Err(RpcError::MethodCallError(SyncStatusError::ContextNotFound));
        };
        let is_initialized = *context.root_hash != [0; 32];

        // The sync run-loop's best-effort snapshot supplies the "why" when the
        // context isn't initialized: syncing vs backing off vs nothing
        // recorded. Absent snapshot (never dispatched, or already settled) maps
        // to "idle".
        let snapshot = state.node_client.sync_status(context_id).await?;

        let (sync_state, retry_in_secs, failure_count, last_error) = match snapshot {
            Some(snap) => {
                let (state_str, retry) = match snap.phase {
                    SyncPhase::Idle => ("idle", None),
                    SyncPhase::Syncing => ("syncing", None),
                    SyncPhase::BackingOff { retry_in_secs } => ("backingOff", Some(retry_in_secs)),
                };
                (
                    state_str.to_owned(),
                    retry,
                    snap.failure_count,
                    snap.last_error,
                )
            }
            None => ("idle".to_owned(), None, 0, None),
        };

        Ok(SyncStatusResponse::new(
            context_id,
            is_initialized,
            sync_state,
            retry_in_secs,
            failure_count,
            last_error,
        ))
    }
}
