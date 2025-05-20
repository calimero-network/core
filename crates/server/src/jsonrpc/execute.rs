use std::sync::Arc;

use calimero_server_primitives::jsonrpc::{ExecutionError, ExecutionRequest, ExecutionResponse};
use tracing::{error, info};

use super::{Request, RpcError, ServiceState};

impl Request for ExecutionRequest {
    type Response = ExecutionResponse;
    type Error = ExecutionError;

    async fn handle(
        self,
        state: Arc<ServiceState>,
    ) -> Result<Self::Response, RpcError<Self::Error>> {
        let context_id = self.context_id;
        let executor_id = self.executor_public_key;

        handle(self, &state).await.map_err(|err| {
            error!(%context_id, %executor_id, %err, "Failed to execute request");

            RpcError::MethodCallError(err)
        })
    }
}

async fn handle(
    request: ExecutionRequest,
    state: &ServiceState,
) -> Result<ExecutionResponse, ExecutionError> {
    let args =
        serde_json::to_vec(&request.args_json).map_err(|err| ExecutionError::SerdeError {
            message: err.to_string(),
        })?;

    let outcome = state
        .ctx_client
        .execute(
            &request.context_id,
            request.method,
            args,
            &request.executor_public_key,
            request.substitute,
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
        return Ok(ExecutionResponse::new(None));
    };

    let returns = serde_json::from_slice(&returns).map_err(|err| ExecutionError::SerdeError {
        message: err.to_string(),
    })?;

    Ok(ExecutionResponse::new(Some(returns)))
}
