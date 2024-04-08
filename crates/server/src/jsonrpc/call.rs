use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{CallError, CallRequest, CallResponse};

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
        Ok(Some(output)) => match serde_json::from_str::<serde_json::Value>(&output) {
            Ok(v) => Ok(CallResponse { output: Some(v) }),
            Err(err) => eyre::bail!(CallError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(CallResponse { output: None }),
        Err(err) => eyre::bail!(CallError::ExecutionError {
            message: err.to_string()
        }),
    }
}
