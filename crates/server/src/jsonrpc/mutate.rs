use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{MutateError, MutateRequest, MutateResponse};

use crate::jsonrpc;

jsonrpc::mount_method!(MutateRequest-> Result<MutateResponse, MutateError>, handle);

async fn handle(
    request: MutateRequest,
    state: Arc<jsonrpc::ServiceState>,
) -> eyre::Result<MutateResponse> {
    let args = match serde_json::to_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            eyre::bail!(MutateError::SerdeError {
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
            Ok(v) => Ok(MutateResponse { output: Some(v) }),
            Err(err) => eyre::bail!(MutateError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(MutateResponse { output: None }),
        Err(err) => match err {
            jsonrpc::CallError::UpstreamCallError(err) => eyre::bail!(MutateError::CallError(err)),
            jsonrpc::CallError::UpstreamFunctionCallError(message) => {
                eyre::bail!(MutateError::FunctionCallError(message))
            }
            jsonrpc::CallError::InternalError(err) => eyre::bail!(err),
        },
    }
}
