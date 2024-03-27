use std::sync::Arc;

use crate::jsonrpc;
use crate::jsonrpc::{JsonRpcMethod, JsonRpcRequest};

impl JsonRpcRequest for calimero_primitives::server::jsonrpc::CallRequest {
    type Response = calimero_primitives::server::jsonrpc::CallResponse;
    type Error = calimero_primitives::server::jsonrpc::CallError;

    async fn handle(
        self,
        state: Arc<jsonrpc::ServiceState>,
    ) -> Result<Self::Response, Self::Error> {
        let args = match serde_json::to_vec(&self.args_json) {
            Ok(args) => args,
            Err(e) => {
                return Err(calimero_primitives::server::jsonrpc::CallError::SerdeError(
                    e.to_string(),
                ))
            }
        };

        match jsonrpc::call(
            state.server_sender.clone(),
            self.application_id,
            self.method,
            args,
            false,
        )
        .await
        {
            Ok(result) => Ok(calimero_primitives::server::jsonrpc::CallResponse { output: result }),
            Err(e) => {
                Err(calimero_primitives::server::jsonrpc::CallError::ExecutionError(e.to_string()))
            }
        }
    }
}

impl JsonRpcMethod for calimero_primitives::server::jsonrpc::CallRequest {
    async fn handle(
        self,
        state: Arc<jsonrpc::ServiceState>,
    ) -> Result<
        calimero_primitives::server::jsonrpc::ResponseResult,
        calimero_primitives::server::jsonrpc::ResponseError,
    > {
        match JsonRpcRequest::handle(self, state.clone()).await {
            Ok(response) => Ok(calimero_primitives::server::jsonrpc::ResponseResult::Call(
                response,
            )),
            Err(error) => Err(calimero_primitives::server::jsonrpc::ResponseError::Call(
                error,
            )),
        }
    }
}
