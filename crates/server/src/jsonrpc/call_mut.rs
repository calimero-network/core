use std::sync::Arc;

use calimero_primitives::server::jsonrpc::{CallMutError, CallMutRequest, CallMutResponse};

use crate::jsonrpc;

jsonrpc::mount_method!(CallMutRequest-> Result<CallMutResponse, CallMutError>, handle);

async fn handle(
    request: CallMutRequest,
    state: Arc<jsonrpc::ServiceState>,
) -> eyre::Result<CallMutResponse> {
    let args = match serde_json::to_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            eyre::bail!(CallMutError::SerdeError {
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
        Ok(output) => Ok(CallMutResponse { output }),
        Err(err) => eyre::bail!(CallMutError::ExecutionError {
            message: err.to_string()
        }),
    }
}
