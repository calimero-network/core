use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use tracing::{error, warn};

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

        // Determine the caller identity from the auth extensions injected by
        // AuthGuardService:
        //   AuthenticatedKey       → verified public key, membership check runs
        //   AuthenticatedNodeOwner → non-key auth (e.g. embedded username/password),
        //                            caller is the node owner, check skipped
        //   neither                → no-auth mode (auth_service = None); warn so a
        //                            misconfigured guard is visible in production logs
        // Three auth paths:
        //   AuthenticatedKey       → token with a verified Ed25519 key; membership check runs
        //   AuthenticatedNodeOwner → non-key auth (embedded username/password); skip check
        //   neither                → no-auth mode where the auth guard is intentionally disabled
        //                            (auth_service = None in config). Warn so a misconfigured
        //                            guard is visible in production logs. This is a deliberate
        //                            deployment choice, not a silent security bypass — operators
        //                            must explicitly omit auth config to reach this path.
        let caller = match auth_key.as_ref() {
            Some(k) => CallerIdentity::Key(&k.0),
            None => {
                if auth_node_owner.is_none() {
                    warn!("No auth extensions present on JSON-RPC execute request — auth guard may not be running");
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
