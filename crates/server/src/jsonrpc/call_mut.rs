use std::sync::Arc;

use calimero_primitives::server::jsonrpc as jsonrpc_primitives;

use crate::jsonrpc;

impl jsonrpc::Request for jsonrpc_primitives::CallMutRequest {
    type Response = jsonrpc_primitives::CallMutResponse;
    type Error = jsonrpc_primitives::CallMutError;

    async fn handle(
        self,
        state: Arc<jsonrpc::ServiceState>,
    ) -> Result<Self::Response, jsonrpc::RpcError<Self::Error>> {
        match handle(self, state).await {
            Ok(response) => Ok(response),
            Err(err) => match err.downcast::<Self::Error>() {
                Ok(err) => Err(jsonrpc::RpcError::MethodCallError(err)),
                Err(err) => Err(jsonrpc::RpcError::InternalError(err)),
            },
        }
    }
}

async fn handle(
    request: jsonrpc_primitives::CallMutRequest,
    state: Arc<jsonrpc::ServiceState>,
) -> eyre::Result<jsonrpc_primitives::CallMutResponse> {
    let args = match serde_json::to_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            eyre::bail!(jsonrpc_primitives::CallMutError::SerdeError {
                message: err.to_string()
            })
        }
    };

    match jsonrpc::call(
        state.server_sender.clone(),
        request.application_id,
        request.method,
        args,
        true,
    )
    .await
    {
        Ok(output) => Ok(jsonrpc_primitives::CallMutResponse { output }),
        Err(err) => eyre::bail!(jsonrpc_primitives::CallMutError::ExecutionError {
            message: err.to_string()
        }),
    }
}
