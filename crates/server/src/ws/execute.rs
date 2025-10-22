use std::sync::Arc;

use calimero_server_primitives::jsonrpc::ExecutionError;
use calimero_server_primitives::ws::{ExecuteRequest, ExecuteResponse};
use eyre::Result as EyreResult;
use tracing::info;

use crate::ws::{mount_method, ConnectionState, ServiceState};

mount_method!(ExecuteRequest -> Result<ExecuteResponse, ExecutionError>, handle);

async fn handle(
    request: ExecuteRequest,
    state: Arc<ServiceState>,
    _connection_state: ConnectionState,
) -> EyreResult<ExecuteResponse> {
    let context_id = request.context_id;
    let executor_id = request.executor_public_key;

    let args = serde_json::to_vec(&request.args_json).map_err(|err| ExecutionError::SerdeError {
        message: err.to_string(),
    })?;

    let outcome = state
        .ctx_client
        .execute(
            &request.context_id,
            &request.executor_public_key,
            request.method,
            args,
            request.substitute,
            None,
        )
        .await
        .map_err(ExecutionError::ExecuteError)?;

    let x = outcome.logs.len().checked_ilog10().unwrap_or(0) as usize + 1;
    for (i, log) in outcome.logs.iter().enumerate() {
        info!("execution log {i:>x$}| {}", log);
    }

    let Some(returns) = outcome
        .returns
        .map_err(|e| ExecutionError::FunctionCallError(e.to_string()))?
    else {
        return Ok(ExecuteResponse { output: None });
    };

    let returns = serde_json::from_slice(&returns).map_err(|err| ExecutionError::SerdeError {
        message: err.to_string(),
    })?;

    info!(%context_id, %executor_id, "Execute request completed successfully");

    Ok(ExecuteResponse {
        output: Some(returns),
    })
}

