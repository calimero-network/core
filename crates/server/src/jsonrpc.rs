use std::sync::Arc;

use axum::routing::{post, MethodRouter};
use axum::{extract, Extension, Json};
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::info;

use crate::ServerSender;

mod call;
mod callmut;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub(crate) struct ServiceState {
    server_sender: ServerSender,
}

pub(crate) fn service(
    config: &crate::config::ServerConfig,
    server_sender: ServerSender,
) -> eyre::Result<Option<(&'static str, MethodRouter)>> {
    let _config = match &config.jsonrpc {
        Some(config) if config.enabled => config,
        _ => {
            info!("JSON RPC server is disabled");
            return Ok(None);
        }
    };

    let path = "/jsonrpc"; // todo! source from config

    for listen in config.listen.iter() {
        info!("JSON RPC server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState { server_sender });

    Ok(Some((path, post(handle_request).layer(Extension(state)))))
}

async fn handle_request(
    Extension(state): Extension<Arc<ServiceState>>,
    extract::Json(request): extract::Json<calimero_primitives::server::jsonrpc::Request>,
) -> Json<calimero_primitives::server::jsonrpc::Response> {
    let result = match request.payload {
        calimero_primitives::server::jsonrpc::RequestPayload::Call(request) => {
            JsonRpcMethod::handle(request, state).await
        }
        calimero_primitives::server::jsonrpc::RequestPayload::CallMut(request) => {
            JsonRpcMethod::handle(request, state).await
        }
    };

    let (result, error) = match result {
        Ok(result) => (Some(result), None),
        Err(error) => (None, Some(error)),
    };

    let response = calimero_primitives::server::jsonrpc::Response {
        jsonrpc: request.jsonrpc,
        result,
        error,
        id: request.id,
    };
    Json(response)
}

pub(crate) async fn call(
    sender: crate::ServerSender,
    app_id: String,
    method: String,
    args: Vec<u8>,
    writes: bool,
) -> eyre::Result<Option<String>> {
    let (result_sender, result_receiver) = oneshot::channel();

    sender
        .send((app_id, method, args, writes, result_sender))
        .await?;

    let outcome = result_receiver.await?;

    for log in outcome.logs {
        info!("RPC log: {}", log);
    }

    match outcome.returns? {
        Some(returns) => Ok(Some(String::from_utf8(returns)?)),
        None => Ok(None),
    }
}

pub(crate) trait JsonRpcRequest {
    type Response;
    type Error;

    async fn handle(self, state: Arc<ServiceState>) -> Result<Self::Response, Self::Error>;
}

pub(crate) trait JsonRpcMethod {
    async fn handle(
        self,
        state: Arc<ServiceState>,
    ) -> Result<
        calimero_primitives::server::jsonrpc::ResponseResult,
        calimero_primitives::server::jsonrpc::ResponseError,
    >;
}
