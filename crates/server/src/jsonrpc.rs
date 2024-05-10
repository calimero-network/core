use std::sync::Arc;

use axum::routing::{post, MethodRouter};
use axum::{extract, Extension, Json};
use calimero_server_primitives::jsonrpc as jsonrpc_primitives;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::oneshot;
use tracing::{debug, error, info};

mod mutate;
mod query;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub(crate) struct ServiceState {
    server_sender: calimero_node_primitives::ServerSender,
}

pub(crate) fn service(
    config: &crate::config::ServerConfig,
    server_sender: calimero_node_primitives::ServerSender,
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
    extract::Json(request): extract::Json<jsonrpc_primitives::Request<serde_json::Value>>,
) -> Json<jsonrpc_primitives::Response> {
    debug!(?request, "Received request");
    let body = match serde_json::from_value::<jsonrpc_primitives::RequestPayload>(request.payload) {
        Ok(payload) => match payload {
            jsonrpc_primitives::RequestPayload::Query(request) => {
                request.handle(state).await.to_res_body()
            }
            jsonrpc_primitives::RequestPayload::Mutate(request) => {
                request.handle(state).await.to_res_body()
            }
        },
        Err(err) => {
            error!(%err, "Failed to deserialize jsonrpc_primitives::RequestPayload");

            jsonrpc_primitives::ResponseBody::Error(
                jsonrpc_primitives::ResponseBodyError::ServerError(
                    jsonrpc_primitives::ServerResponseError::ParseError(err.to_string()),
                ),
            )
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

#[derive(Debug, Error)]
#[error("CallError")]
pub(crate) enum CallError {
    UpstreamCallError(calimero_node_primitives::CallError),
    UpstreamFunctionCallError(String), // TODO use FunctionCallError from runtime-primitives once they are migrated
    InternalError(eyre::Error),
}

pub(crate) async fn call(
    sender: calimero_node_primitives::ServerSender,
    application_id: calimero_primitives::application::ApplicationId,
    method: String,
    args: Vec<u8>,
    writes: bool,
) -> Result<Option<String>, CallError> {
    let (outcome_sender, outcome_receiver) = oneshot::channel();

    sender
        .send((application_id, method, args, writes, outcome_sender))
        .await
        .map_err(|e| CallError::InternalError(eyre::eyre!("Failed to send call message: {}", e)))?;

    match outcome_receiver.await.map_err(|e| {
        CallError::InternalError(eyre::eyre!("Failed to receive call outcome result: {}", e))
    })? {
        Ok(outcome) => {
            for log in outcome.logs {
                info!("RPC log: {}", log);
            }

            let Some(returns) = outcome
                .returns
                .map_err(|e| CallError::UpstreamFunctionCallError(e.to_string()))?
            else {
                return Ok(None);
            };

            Ok(Some(String::from_utf8(returns).map_err(|e| {
                CallError::InternalError(eyre::eyre!(
                    "Failed to convert call result to string: {}",
                    e
                ))
            })?))
        }
        Err(err) => Err(CallError::UpstreamCallError(err)),
    }
}

macro_rules! mount_method {
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

pub(crate) use mount_method;
