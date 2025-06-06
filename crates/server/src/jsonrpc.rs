use std::sync::Arc;

use axum::middleware::from_fn;
use axum::routing::{post, Router};
use axum::{Extension, Json};
use calimero_context_primitives::client::ContextClient;
use calimero_server_primitives::jsonrpc::{
    Request as PrimitiveRequest, RequestPayload, Response as PrimitiveResponse, ResponseBody,
    ResponseBodyError, ResponseBodyResult, ServerResponseError,
};
use calimero_store::Store;
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use crate::config::ServerConfig;
use crate::middleware::dev_auth::dev_mode_auth;
use crate::middleware::jwt::JwtLayer;

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
    ctx_client: ContextClient,
}

pub(crate) fn service(
    config: &ServerConfig,
    ctx_client: ContextClient,
    datastore: Store,
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

    let state = Arc::new(ServiceState { ctx_client });
    let handler = post(handle_request).layer(Extension(Arc::clone(&state)));

    let mut router = Router::new().route("/", handler.clone());

    if jsonrpc_config.auth_enabled {
        router = router.route_layer(JwtLayer::new(datastore));
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
    Json(request): Json<PrimitiveRequest<serde_json::Value>>,
) -> Json<PrimitiveResponse> {
    debug!(id=?request.id, payload=%request.payload, "Received request");

    let body = match serde_json::from_value(request.payload) {
        Ok(payload) => match payload {
            RequestPayload::Execute(request) => request.handle(state).await.to_res_body(),
        },
        Err(err) => {
            debug!(%err, "Failed to deserialize RequestPayload");

            ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::ParseError(err.to_string()),
            ))
        }
    };

    if let ResponseBody::Error(err) = &body {
        debug!(id=?request.id, %err, "request handling failed");
    }

    PrimitiveResponse::new(request.jsonrpc, request.id, body).into()
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
    InternalError(eyre::Report),
}

impl<E, X: Into<eyre::Report>> From<X> for RpcError<E> {
    fn from(err: X) -> Self {
        RpcError::InternalError(err.into())
    }
}

trait ToResponseBody {
    fn to_res_body(self) -> ResponseBody;
}

impl<T: Serialize, E: Serialize> ToResponseBody for Result<T, RpcError<E>> {
    fn to_res_body(self) -> ResponseBody {
        let err = match self {
            Ok(r) => match serde_json::to_value(r) {
                Ok(v) => return ResponseBody::Result(ResponseBodyResult(v)),
                Err(err) => err.into(),
            },
            Err(RpcError::MethodCallError(err)) => match serde_json::to_value(err) {
                Ok(v) => return ResponseBody::Error(ResponseBodyError::HandlerError(v)),
                Err(err) => err.into(),
            },
            Err(RpcError::InternalError(err)) => err,
        };

        ResponseBody::Error(ResponseBodyError::ServerError(
            ServerResponseError::InternalError { err: Some(err) },
        ))
    }
}
