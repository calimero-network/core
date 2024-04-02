use std::sync::Arc;

use axum::routing::{post, MethodRouter};
use axum::{extract, Extension, Json};
use calimero_server_primitives::jsonrpc as jsonrpc_primitives;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{error, info};

use crate::ServerSender;

mod call;
mod call_mut;

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
    extract::Json(request): extract::Json<jsonrpc_primitives::Request>,
) -> Json<jsonrpc_primitives::Response> {
    let body = match request.payload {
        jsonrpc_primitives::RequestPayload::Call(request) => {
            request.handle(state).await.to_res_body()
        }
        jsonrpc_primitives::RequestPayload::CallMut(request) => {
            request.handle(state).await.to_res_body()
        }
    };

    if let jsonrpc_primitives::ResponseBody::Error(err) = &body {
        error!(?err, "Failed to execute JSON RPC method");
    }

    let response = jsonrpc_primitives::Response {
        jsonrpc: request.jsonrpc,
        body,
        id: request.id,
    };
    Json(response)
}

pub(crate) trait Request {
    type Response;
    type Error;

    async fn handle(
        self,
        state: Arc<ServiceState>,
    ) -> Result<Self::Response, RpcError<Self::Error>>;
}

pub enum RpcError<E> {
    MethodCallError(E),
    InternalError(eyre::Error),
}

trait ToResponseBody {
    fn to_res_body(self) -> jsonrpc_primitives::ResponseBody;
}

impl<T: Serialize, E: Serialize> ToResponseBody for Result<T, RpcError<E>> {
    fn to_res_body(self) -> jsonrpc_primitives::ResponseBody {
        match self {
            Ok(r) => match serde_json::to_value(r) {
                Ok(v) => jsonrpc_primitives::ResponseBody::Result(
                    jsonrpc_primitives::ResponseBodyResult(v),
                ),
                Err(err) => jsonrpc_primitives::ResponseBody::Error(
                    jsonrpc_primitives::ResponseBodyError::ServerError(
                        jsonrpc_primitives::ServerResponseError::InternalError {
                            err: Some(err.into()),
                        },
                    ),
                ),
            },
            Err(RpcError::MethodCallError(err)) => match serde_json::to_value(err) {
                Ok(v) => jsonrpc_primitives::ResponseBody::Error(
                    jsonrpc_primitives::ResponseBodyError::HandlerError(v),
                ),
                Err(err) => jsonrpc_primitives::ResponseBody::Error(
                    jsonrpc_primitives::ResponseBodyError::ServerError(
                        jsonrpc_primitives::ServerResponseError::InternalError {
                            err: Some(err.into()),
                        },
                    ),
                ),
            },
            Err(RpcError::InternalError(err)) => jsonrpc_primitives::ResponseBody::Error(
                jsonrpc_primitives::ResponseBodyError::ServerError(
                    jsonrpc_primitives::ServerResponseError::InternalError { err: Some(err) },
                ),
            ),
        }
    }
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

    let Some(returns) = outcome.returns? else {
        return Ok(None);
    };

    Ok(Some(String::from_utf8(returns)?))
}

macro_rules! _mount_method {
    ($request:ident -> Result<$response:ident, $error:ident>, $handle:path) => {
        impl crate::jsonrpc::Request for $request {
            type Response = $response;
            type Error = $error;

            async fn handle(
                self,
                state: std::sync::Arc<crate::jsonrpc::ServiceState>,
            ) -> std::result::Result<Self::Response, crate::jsonrpc::RpcError<Self::Error>> {
                match $handle(self, state).await {
                    Ok(response) => Ok(response),
                    Err(err) => match err.downcast::<Self::Error>() {
                        Ok(err) => Err(jsonrpc::RpcError::MethodCallError(err)),
                        Err(err) => Err(jsonrpc::RpcError::InternalError(err)),
                    },
                }
            }
        }
    };
}

pub(crate) use _mount_method as mount_method;
