use std::sync::Arc;

use axum::routing::{post, Router};
use axum::{Extension, Json, middleware::from_fn};
use calimero_node_primitives::{CallError as PrimitiveCallError, ExecutionRequest, ServerSender};
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::{
    Request as PrimitiveRequest, RequestPayload, Response as PrimitiveResponse, ResponseBody,
    ResponseBodyError, ResponseBodyResult, ServerResponseError,
};
use calimero_store::Store;
use eyre::{eyre, Error as EyreError};
use serde::{Deserialize, Serialize};
use serde_json::{from_value as from_json_value, to_value as to_json_value, Value};
use thiserror::Error as ThisError;
use tokio::sync::oneshot;
use tracing::{debug, error, info};

use crate::config::ServerConfig;
use crate::middleware::jwt::JwtLayer;
use crate::middleware::dev_auth::dev_mode_auth;

mod execute;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct JsonRpcConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
    #[serde(skip)]
    pub auth_enabled: bool,
}

impl JsonRpcConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self {
            enabled,
            auth_enabled: false,
        }
    }
}

pub(crate) struct ServiceState {
    server_sender: ServerSender,
}

pub(crate) fn service(
    config: &ServerConfig,
    server_sender: ServerSender,
    store: Store,
) -> Option<(&'static str, Router)> {
    let jsonrpc_config = match &config.jsonrpc {
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

    let state = Arc::new(ServiceState { server_sender });
    let handler = post(handle_request).layer(Extension(state.clone()));

    let mut router = Router::new().route("/", handler.clone());

    if jsonrpc_config.auth_enabled {
        router = router.route_layer(JwtLayer::new(store));
    }

    let mut dev_router = Router::new()
        .route("/", handler)
        .layer(Extension(Arc::clone(&state)));

    if jsonrpc_config.auth_enabled {
        dev_router = dev_router.route_layer(from_fn(dev_mode_auth));
    }

    router = router.nest("/dev", dev_router);

    Some((path, router))
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
    CallError(PrimitiveCallError),
    #[error("function call error: {0}")]
    FunctionCallError(String), // TODO use FunctionCallError from runtime-primitives once they are migrated
    #[error(transparent)]
    InternalError(EyreError),
}

pub(crate) async fn call(
    sender: ServerSender,
    context_id: ContextId,
    method: String,
    args: Vec<u8>,
    executor_public_key: PublicKey,
) -> Result<Option<String>, CallError> {
    let (outcome_sender, outcome_receiver) = oneshot::channel();

    sender
        .send(ExecutionRequest::new(
            context_id,
            method,
            args,
            executor_public_key,
            outcome_sender,
        ))
        .await
        .map_err(|e| CallError::InternalError(eyre!("Failed to send call message: {}", e)))?;

    match outcome_receiver.await.map_err(|e| {
        CallError::InternalError(eyre!("Failed to receive call outcome result: {}", e))
    })? {
        Ok(outcome) => {
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
        Err(err) => Err(CallError::CallError(err)),
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
