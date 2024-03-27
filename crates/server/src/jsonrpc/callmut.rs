use std::sync::Arc;

use crate::jsonrpc;
use crate::jsonrpc::{JsonRpcMethod, JsonRpcRequest};

impl JsonRpcRequest for calimero_primitives::server::jsonrpc::CallMutRequest {
    type Response = calimero_primitives::server::jsonrpc::CallMutResponse;
    type Error = calimero_primitives::server::jsonrpc::CallMutError;

    async fn handle(
        self,
        state: Arc<jsonrpc::ServiceState>,
    ) -> Result<Self::Response, Self::Error> {
        let args = match serde_json::to_vec(&self.args_json) {
            Ok(args) => args,
            Err(e) => {
                return Err(
                    calimero_primitives::server::jsonrpc::CallMutError::SerdeError(e.to_string()),
                )
            }
        };

        match jsonrpc::call(
            state.server_sender.clone(),
            self.application_id,
            self.method,
            args,
            true,
        )
        .await
        {
            Ok(result) => {
                Ok(calimero_primitives::server::jsonrpc::CallMutResponse { output: result })
            }
            Err(e) => Err(
                calimero_primitives::server::jsonrpc::CallMutError::ExecutionError(e.to_string()),
            ),
        }
    }
}

impl JsonRpcMethod for calimero_primitives::server::jsonrpc::CallMutRequest {
    async fn handle(
        self,
        state: Arc<jsonrpc::ServiceState>,
    ) -> Result<
        calimero_primitives::server::jsonrpc::ResponseResult,
        calimero_primitives::server::jsonrpc::ResponseError,
    > {
        match JsonRpcRequest::handle(self, state.clone()).await {
            Ok(response) => {
                Ok(calimero_primitives::server::jsonrpc::ResponseResult::CallMut(response))
            }
            Err(error) => Err(calimero_primitives::server::jsonrpc::ResponseError::CallMut(error)),
        }
    }
}
