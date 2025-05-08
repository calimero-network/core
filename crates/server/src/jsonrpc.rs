use std::sync::Arc;

use axum::routing::{post, MethodRouter};
use axum::{Extension, Json};
use calimero_context_primitives::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    ExecuteError, Request as PrimitiveRequest, RequestPayload, Response as PrimitiveResponse, ResponseBody, ResponseBodyError, ResponseBodyResult, ServerResponseError
};
use eyre::{eyre, Error as EyreError};
use serde::{Deserialize, Serialize};
use serde_json::{from_value as from_json_value, to_value as to_json_value, Value};
use thiserror::Error as ThisError;
use tracing::{debug, error, info};

use crate::config::ServerConfig;

mod execute;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct JsonRpcConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

impl JsonRpcConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

pub(crate) struct ServiceState {
    node_client: NodeClient,
    ctx_client: ContextClient,
}

pub(crate) fn service(
    config: &ServerConfig,
    node_client: NodeClient,
    ctx_client: ContextClient,
) -> Option<(&'static str, MethodRouter)> {
    let _config = match &config.jsonrpc {
        Some(config) if config.enabled => config,
        _ => {
            info!("JSON RPC server is disabled");
            return None;
        }
    };

    let path = "/jsonrpc"; // todo! source from config

    for listen in &config.listen {
        info!("JSON RPC server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState { node_client, ctx_client });

    Some((path, post(handle_request).layer(Extension(state))))
}

async fn handle_request(
    Extension(state): Extension<Arc<ServiceState>>,
    Json(request): Json<PrimitiveRequest<Value>>,
) -> Json<PrimitiveResponse> {
    debug!(?request, "Received request");
    let body = match from_json_value::<RequestPayload>(request.payload) {
        Ok(payload) => match payload {
            RequestPayload::Execute(request) => request.handle(state).await.to_res_body(),
        },
        Err(err) => {
            error!(%err, "Failed to deserialize RequestPayload");

            ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::ParseError(err.to_string()),
            ))
        }
    };

    let response = PrimitiveResponse::new(request.jsonrpc, request.id, body);
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

#[derive(Debug)]
#[non_exhaustive]
pub enum RpcError<E> {
    MethodCallError(E),
    InternalError(EyreError),
}

trait ToResponseBody {
    fn to_res_body(self) -> ResponseBody;
}

impl<T: Serialize, E: Serialize> ToResponseBody for Result<T, RpcError<E>> {
    fn to_res_body(self) -> ResponseBody {
        match self {
            Ok(r) => match to_json_value(r) {
                Ok(v) => ResponseBody::Result(ResponseBodyResult(v)),
                Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError {
                        err: Some(err.into()),
                    },
                )),
            },
            Err(RpcError::MethodCallError(err)) => match to_json_value(err) {
                Ok(v) => ResponseBody::Error(ResponseBodyError::HandlerError(v)),
                Err(err) => ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError {
                        err: Some(err.into()),
                    },
                )),
            },
            Err(RpcError::InternalError(err)) => {
                ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError { err: Some(err) },
                ))
            }
        }
    }
}

#[derive(Debug, ThisError)]
#[expect(clippy::enum_variant_names, reason = "Acceptable here")]
pub(crate) enum CallError {
    #[error(transparent)]
    CallError(ExecuteError),
    #[error("function call error: {0}")]
    FunctionCallError(String), // TODO use FunctionCallError from runtime-primitives once they are migrated
    #[error(transparent)]
    InternalError(EyreError),
}

pub(crate) async fn call(
    ctx_client: ContextClient,
    context_id: ContextId,
    method: String,
    args: Vec<u8>,
    executor_public_key: PublicKey,
) -> Result<Option<String>, CallError> {

    let outcome = ctx_client.execute(&context_id, method, args, &executor_public_key)
        .await
        .map_err(|e| CallError::InternalError(eyre!("Failed to send call message: {}", e)))?;
        
    let x = outcome.logs.len().checked_ilog10().unwrap_or(0) as usize + 1;
    for (i, log) in outcome.logs.iter().enumerate() {
        info!("execution log {i:>x$}| {}", log);
    }

    let Some(returns) = outcome
        .returns
        .map_err(|e| CallError::FunctionCallError(e.to_string()))?
    else {
        return Ok(None);
    };

    Ok(Some(String::from_utf8(returns).map_err(|e| {
        CallError::InternalError(eyre!("Failed to convert call result to string: {}", e))
    })?))  
}

macro_rules! mount_method {
    ($request:ident -> Result<$response:ident, $error:ident>, $handle:path) => {
        impl crate::jsonrpc::Request for $request {
            type Response = $response;
            type Error = $error;

            async fn handle(
                self,
                state: std::sync::Arc<crate::jsonrpc::ServiceState>,
            ) -> core::result::Result<Self::Response, crate::jsonrpc::RpcError<Self::Error>> {
                match $handle(self, state).await {
                    Ok(response) => Ok(response),
                    Err(err) => match err.downcast::<Self::Error>() {
                        Ok(err) => Err(crate::jsonrpc::RpcError::MethodCallError(err)),
                        Err(err) => Err(crate::jsonrpc::RpcError::InternalError(err)),
                    },
                }
            }
        }
    };
}

pub(crate) use mount_method;
