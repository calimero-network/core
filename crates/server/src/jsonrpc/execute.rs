use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use tracing::error;

use super::{Request, RpcError, ServiceState};
use crate::auth::AuthenticatedKey;
use crate::execute::{execute_request, CallerIdentity};

impl Request for ExecutionRequest {
    type Response = ExecutionResponse;
    type Error = ExecutionError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
        auth_key: Option<AuthenticatedKey>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        let context_id = self.context_id;

        let caller = match auth_key.as_ref() {
            Some(k) => CallerIdentity::Key(&k.0),
            None => CallerIdentity::NodeOwner,
        };
        execute_request(&state.ctx_client, caller, self)
            .await
            .map_err(|err| {
                error!(%context_id, %err, "Failed to execute request");

                RpcError::MethodCallError(err)
            })
    }
}
