use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecuteError, ExecuteRequest, ExecuteResponse};
use eyre::{bail, Result as EyreResult};
use serde_json::{from_str as from_json_str, to_vec as to_json_vec, Value};
use tracing::error;

use crate::jsonrpc::{call, mount_method, CallError, ServiceState};

mount_method!(ExecuteRequest-> Result<ExecuteResponse, ExecuteError>, handle);

async fn handle(request: ExecuteRequest, state: Arc<ServiceState>) -> EyreResult<ExecuteResponse> {
    let args = match to_json_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            bail!(ExecuteError::SerdeError {
                message: err.to_string()
            })
        }
    };

    match call(
        state.ctx_client.clone(),
        request.context_id,
        request.method,
        args,
        request.executor_public_key,
    )
    .await
    {
        Ok(Some(output)) => match from_json_str::<Value>(&output) {
            Ok(v) => Ok(ExecuteResponse::new(Some(v))),
            Err(err) => bail!(ExecuteError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(ExecuteResponse::new(None)),
        Err(err) => {
            error!(%err, "Failed to execute JSON RPC method");

            match err {
                CallError::CallError(err) => {
                    bail!(err)
                }
                CallError::FunctionCallError(message) => {
                    bail!(ExecuteError::FunctionCallError(message))
                }
                CallError::InternalError(err) => bail!(err),
            }
        }
    }
}
