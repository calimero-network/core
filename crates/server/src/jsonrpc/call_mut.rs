use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{CallMutError, CallMutRequest, CallMutResponse};

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
        Ok(Some(output)) => match serde_json::from_str::<serde_json::Value>(&output) {
            Ok(v) => Ok(CallMutResponse { output: Some(v) }),
            Err(err) => eyre::bail!(CallMutError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(CallMutResponse { output: None }),
        Err(err) => eyre::bail!(CallMutError::ExecutionError {
            message: err.to_string()
        }),
    }
}
