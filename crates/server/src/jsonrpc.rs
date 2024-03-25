use std::sync::Arc;

use axum::routing::{post, MethodRouter};
use axum::{extract, Extension, Json};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::ServerSender;

mod service;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

struct ServiceState {
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
    extract::Json(request): extract::Json<calimero_primitives::server::JsonRpcRequest>,
) -> Json<calimero_primitives::server::JsonRpcResponse> {
    let result = match request.method.as_str() {
        "execute" => match request.params {
            Some(params) => match params {
                calimero_primitives::server::JsonRpcRequestParams::Call(params) => {
                    service::handle_execute_method(state.server_sender.clone(), params).await
                }
                _ => Err(eyre::eyre!(
                    "invalid params type, method={}",
                    request.method,
                )),
            },
            None => Err(eyre::eyre!("missing params, method={}", request.method)),
        },
        "read" => match request.params {
            Some(params) => match params {
                calimero_primitives::server::JsonRpcRequestParams::Call(params) => {
                    service::handle_read_method(state.server_sender.clone(), params).await
                }
                _ => Err(eyre::eyre!(
                    "invalid params type, method={}",
                    request.method,
                )),
            },
            None => Err(eyre::eyre!("missing params, method={}", request.method)),
        },
        method => Err(eyre::eyre!(
            "unsupported RPC method invoked, method={}",
            method,
        )),
    };

    let (result, error) = match result {
        Ok(result) => (result, None),
        Err(e) => (
            None,
            Some(calimero_primitives::server::JsonRpcResponseError {
                code: 1,
                message: e.to_string(),
            }),
        ),
    };

    let response = calimero_primitives::server::JsonRpcResponse {
        jsonrpc: request.jsonrpc,
        result,
        error,
        id: request.id,
    };
    Json(response)
}
