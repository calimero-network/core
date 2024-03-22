use std::sync::Arc;

use axum::routing::{post, MethodRouter};
use axum::Json;
use axum::{extract, Extension};
use tracing::info;

use crate::ServerSender;

mod service;
struct ServiceState {
    server_sender: ServerSender,
}

pub(crate) fn service(
    config: &crate::config::ServerConfig,
    server_sender: ServerSender,
) -> eyre::Result<Option<(&'static str, MethodRouter)>> {
    let path = "/rpc"; // todo! source from config

    for listen in config.listen.iter() {
        info!("RPC server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState { server_sender });

    Ok(Some((path, post(handle_request).layer(Extension(state)))))
}

async fn handle_request(
    Extension(state): Extension<Arc<ServiceState>>,
    extract::Json(request): extract::Json<calimero_primitives::server::JsonRpcRequest>,
) -> Json<calimero_primitives::server::JsonRpcResponse> {
    let result = match request.method.as_str() {
        "execute" => service::handle_execute_method(state.server_sender.clone()).await,
        "read" => service::handle_read_method(state.server_sender.clone()).await,
        method => Err(eyre::eyre!("unsupported RPC method invoked: {}", method)),
    };

    let (result, err) = match result {
        Ok(_) => ("", None),
        Err(e) => (
            "",
            Some(calimero_primitives::server::JsonRpcResponseError {
                code: 1,
                message: e.to_string(),
            }),
        ),
    };

    let response = calimero_primitives::server::JsonRpcResponse {
        jsonrpc: request.jsonrpc,
        result: result.to_string(),
        error: err,
        id: request.id,
    };
    Json(response)
}
