use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecuteError, ExecuteRequest, ExecuteResponse};
use tracing::{error, info};

use super::{Request, RpcError, ServiceState};

impl Request for ExecuteRequest {
    type Response = ExecuteResponse;
    type Error = ExecuteError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        handle(self, &state).await.map_err(|err| {
            error!(?err, "Failed to execute JSON RPC method");

            RpcError::MethodCallError(err)
        })
    }
}

async fn handle(
    request: ExecuteRequest,
    state: &ServiceState,
) -> Result<ExecuteResponse, ExecuteError> {
    let args = serde_json::to_vec(&request.args_json).map_err(|err| ExecuteError::SerdeError {
        message: err.to_string(),
    })?;

    let outcome = state
        .ctx_client
        .execute(
            &request.context_id,
            request.method,
            args,
            &request.executor_public_key,
        )
        .await
        .map_err(ExecuteError::ExecuteError)?;

    let x = outcome.logs.len().checked_ilog10().unwrap_or(0) as usize + 1;
    for (i, log) in outcome.logs.iter().enumerate() {
        info!("execution log {i:>x$}| {}", log);
    }

    let Some(returns) = outcome
        .returns
        .map_err(|e| ExecuteError::FunctionCallError(e.to_string()))?
    else {
        return Ok(ExecuteResponse::new(None));
    };

    let returns = serde_json::from_slice(&returns).map_err(|err| ExecuteError::SerdeError {
        message: err.to_string(),
    })?;

    Ok(ExecuteResponse::new(Some(returns)))
}
