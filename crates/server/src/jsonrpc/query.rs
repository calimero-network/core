use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{QueryError, QueryRequest, QueryResponse};
use eyre::{bail, Result as EyreResult};
use serde_json::{from_str as from_json_str, to_vec as to_json_vec, Value};

use crate::jsonrpc::{call, mount_method, CallError, ServiceState};

mount_method!(QueryRequest-> Result<QueryResponse, QueryError>, handle);

async fn handle(request: QueryRequest, state: Arc<ServiceState>) -> EyreResult<QueryResponse> {
    let args = match to_json_vec(&request.args_json) {
        Ok(args) => args,
        Err(err) => {
            bail!(QueryError::SerdeError {
                message: err.to_string()
            })
        }
    };

    match call(
        state.server_sender.clone(),
        request.context_id,
        request.method,
        args,
        request.executor_public_key,
    )
    .await
    {
        Ok(Some(output)) => match from_json_str::<Value>(&output) {
            Ok(v) => Ok(QueryResponse::new(Some(v))),
            Err(err) => bail!(QueryError::SerdeError {
                message: err.to_string()
            }),
        },
        Ok(None) => Ok(QueryResponse::new(None)),
        Err(err) => match err {
            CallError::UpstreamCallError(err) => bail!(QueryError::CallError(err)),
            CallError::UpstreamFunctionCallError(message) => {
                bail!(QueryError::FunctionCallError(message))
            }
            CallError::InternalError(err) => bail!(err),
        },
    }
}
