use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use tracing::error;

use super::{Request, RpcError, ServiceState};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};
use crate::execute::execute_request;

impl Request for ExecutionRequest {
    type Response = ExecutionResponse;
    type Error = ExecutionError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
        auth_key: Option<AuthenticatedKey>,
        auth_node_owner: Option<AuthenticatedNodeOwner>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        let context_id = self.context_id;

        // The three auth paths (key / node-owner / no-auth mode) are resolved
        // by the shared `caller_identity` helper — see its doc comment.
        let caller = super::caller_identity(
            &state,
            auth_key.as_ref(),
            auth_node_owner.as_ref(),
            "execute",
        )?;
        execute_request(&state.ctx_client, caller, self)
            .await
            .map_err(|err| {
                error!(%context_id, %err, "Failed to execute request");

                RpcError::MethodCallError(err)
            })
    }
}
