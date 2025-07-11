use std::sync::Arc;

use axum::routing::{post, Router};
use axum::{Extension, Json};
use calimero_context_primitives::client::ContextClient;
use calimero_server_primitives::jsonrpc::{
    Request as PrimitiveRequest, RequestPayload, Response as PrimitiveResponse, ResponseBody,
    ResponseBodyError, ResponseBodyResult, ServerResponseError,
};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

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
    ctx_client: ContextClient,
}

pub(crate) fn service(
    config: &ServerConfig,
    ctx_client: ContextClient,
) -> Option<(&'static str, Router)> {
    let path = "/jsonrpc"; // todo! source from config

    for listen in &config.listen {
        info!("JSON RPC server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState { ctx_client });
    let handler = post(handle_request).layer(Extension(Arc::clone(&state)));

    let router = Router::new().route("/", handler);

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
        debug!(id=?request.id, ?err, "request handling failed");
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
