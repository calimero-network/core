use std::sync::Arc;

use calimero_primitives::server::jsonrpc::{CallError, CallRequest, CallResponse};

use crate::jsonrpc;

jsonrpc::mount_method!(CallRequest-> Result<CallResponse, CallError>, handle);

async fn handle(
    request: CallRequest,
    state: Arc<jsonrpc::ServiceState>,
) -> eyre::Result<CallResponse> {
    let args = match serde_json::to_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            eyre::bail!(CallError::SerdeError {
                message: err.to_string()
            })
        }
    };

    match jsonrpc::call(
        state.server_sender.clone(),
        request.application_id,
        request.method,
        args,
        false,
    )
    .await
    {
        Ok(output) => Ok(CallResponse { output }),
        Err(err) => eyre::bail!(CallError::ExecutionError {
            message: err.to_string()
        }),
    }
}
