use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{MutateError, MutateRequest, MutateResponse};
use eyre::{bail, Result as EyreResult};
use serde_json::{from_str as from_json_str, to_vec as to_json_vec, Value};

use crate::jsonrpc::{call, mount_method, CallError, ServiceState};

mount_method!(MutateRequest-> Result<MutateResponse, MutateError>, handle);

async fn handle(request: MutateRequest, state: Arc<ServiceState>) -> EyreResult<MutateResponse> {
    let args = match to_json_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            bail!(MutateError::SerdeError {
                message: err.to_string()
            })
        }
    };

    match call(
        state.server_sender.clone(),
        request.context_id,
        request.method,
        args,
        true,
        request.executor_public_key,
    )
    .await
    {
        Ok(Some(output)) => match from_json_str::<Value>(&output) {
            Ok(v) => Ok(MutateResponse::new(Some(v))),
            Err(err) => bail!(MutateError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(MutateResponse::new(None)),
        Err(err) => match err {
            CallError::UpstreamCallError(err) => bail!(MutateError::CallError(err)),
            CallError::UpstreamFunctionCallError(message) => {
                bail!(MutateError::FunctionCallError(message))
            }
            CallError::InternalError(err) => bail!(err),
        },
    }
}
