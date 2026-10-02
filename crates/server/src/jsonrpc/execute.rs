use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use tracing::error;

use super::{Request, RpcError, ServiceState};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner, GrantedPermissions};
use crate::execute::execute_request;

impl Request for ExecutionRequest {
    type Response = ExecutionResponse;
    type Error = ExecutionError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
        auth_key: Option<AuthenticatedKey>,
        auth_node_owner: Option<AuthenticatedNodeOwner>,
        granted: Option<GrantedPermissions>,
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
        execute_request(
            &state.ctx_client,
            caller,
            state.auth_enabled,
            granted.as_ref(),
            self,
        )
        .await
        .map_err(|err| {
            error!(%context_id, %err, "Failed to execute request");

            RpcError::MethodCallError(err)
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;
    use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest};
    use calimero_utils_actix::LazyRecipient;
    use serde_json::json;

    use super::super::test_support::state_with;
    use super::super::{Request, RpcError};
    use crate::auth::{AuthenticatedNodeOwner, GrantedPermissions};

    fn granted(permissions: &[&str]) -> GrantedPermissions {
        GrantedPermissions(
            permissions
                .iter()
                .map(|p| (*p).to_owned())
                .collect::<Arc<[_]>>(),
        )
    }

    /// The route admits any `context:execute`, so a grant scoped to `get` used
    /// to reach every method once it was through. A client key, answered as
    /// the node owner, had no membership check to stop it either.
    #[tokio::test]
    async fn a_method_scoped_grant_is_refused_another_method() {
        let t = state_with(true, LazyRecipient::new()).await;
        let request = ExecutionRequest::new(ContextId::from([3; 32]), "set".to_owned(), json!({}));

        let result = request
            .handle(
                t.state.clone(),
                None,
                Some(AuthenticatedNodeOwner),
                Some(granted(&["context:execute[,,get]"])),
            )
            .await;

        match result {
            Err(RpcError::MethodCallError(ExecutionError::FunctionCallError(message))) => {
                assert!(
                    message.contains("does not grant context:execute"),
                    "refused for its grant, before anything is resolved: {message}"
                );
            }
            other => panic!("a grant for `get` must not reach `set`: {other:?}"),
        }
    }
}
