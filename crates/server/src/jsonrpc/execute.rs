use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use tracing::{debug, error, warn};

use super::{Request, RpcError, ServiceState};
use crate::auth::{AuthenticatedKey, AuthenticatedNodeOwner};
use crate::execute::{execute_request, CallerIdentity};

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

        // Three auth paths:
        //   AuthenticatedKey       → token with a verified Ed25519 key; membership check runs
        //   AuthenticatedNodeOwner → non-key auth (embedded username/password); skip check
        //   neither                → no extensions injected; two sub-cases distinguished by
        //                            `state.auth_enabled`:
        //                             - auth enabled  → guard ran but injected nothing; this
        //                               should not happen and indicates a misconfiguration; reject
        //                               the request to avoid silently granting elevated access.
        //                             - auth disabled → intentional no-auth deployment; proceed
        //                               silently at debug level.
        let caller = match auth_key.as_ref() {
            Some(k) => CallerIdentity::Key(&k.0),
            None => {
                if auth_node_owner.is_none() {
                    if state.auth_enabled {
                        warn!("No auth extensions present on JSON-RPC execute request — auth guard may not be running");
                        return Err(RpcError::MethodCallError(
                            ExecutionError::FunctionCallError("authentication required".to_owned()),
                        ));
                    }
                    debug!("No-auth mode: JSON-RPC execute proceeding without membership check");
                }
                CallerIdentity::NodeOwner
            }
        };
        execute_request(&state.ctx_client, caller, self)
            .await
            .map_err(|err| {
                error!(%context_id, %err, "Failed to execute request");

                RpcError::MethodCallError(err)
            })
    }
}
