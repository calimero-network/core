use std::sync::Arc;

use axum::routing::{post, Router};
use axum::{Extension, Json};
use calimero_context_client::client::ContextClient;
use calimero_node_primitives::client::NodeClient;
use calimero_server_primitives::jsonrpc::{
    Request as PrimitiveRequest, RequestPayload, Response as PrimitiveResponse, ResponseBody,
    ResponseBodyError, ResponseBodyResult, ServerResponseError,
};
use calimero_server_primitives::validation::Validate;
use serde::{Deserialize, Serialize};
use tracing::{error, field, info, info_span, Instrument};
use uuid::Uuid;

use crate::config::ServerConfig;

mod execute;
mod sync_status;

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
    node_client: NodeClient,
}

pub(crate) fn service(
    config: &ServerConfig,
    ctx_client: ContextClient,
    node_client: NodeClient,
) -> Option<(String, Router)> {
    // Check if JSON-RPC is configured and enabled
    let _jsonrpc_config = match &config.jsonrpc {
        Some(cfg) if cfg.enabled => cfg,
        _ => {
            info!("JSON RPC server is disabled");
            return None;
        }
    };

    let base_path = "/jsonrpc";

    // Get the node prefix from env var
    let path = if let Ok(prefix) = std::env::var("NODE_PATH_PREFIX") {
        format!("{prefix}{base_path}")
    } else {
        base_path.to_owned()
    };

    for listen in &config.listen {
        info!("JSON RPC server listening on {}/http{{{}}}", listen, path);
    }

    let state = Arc::new(ServiceState {
        ctx_client,
        node_client,
    });
    let handler = post(handle_request).layer(Extension(Arc::clone(&state)));

    let router = Router::new().route("/", handler);

    Some((path, router))
}

async fn handle_request(
    Extension(state): Extension<Arc<ServiceState>>,
    Json(request): Json<PrimitiveRequest<serde_json::Value>>,
) -> Json<PrimitiveResponse> {
    // One correlation id per inbound request. Carried on a span so every log
    // emitted while handling it -- here and in everything we await (execute.rs,
    // the per-line WASM execution logs, ctx_client) -- inherits `request_id`
    // without threading it through any signatures. `context_id`/`method` start
    // empty and are recorded once the payload is parsed.
    let request_id = Uuid::new_v4();
    let span = info_span!(
        "rpc_request",
        %request_id,
        // The client's own JSON-RPC `id` (echoed back in the response) so a
        // caller can line their id up with the server trace. Unlike
        // `request_id` it is client-controlled and may be Null.
        client_id = ?request.id,
        context_id = field::Empty,
        method = field::Empty,
    );

    handle_request_inner(state, request).instrument(span).await
}

async fn handle_request_inner(
    state: Arc<ServiceState>,
    request: PrimitiveRequest<serde_json::Value>,
) -> Json<PrimitiveResponse> {
    let body = match serde_json::from_value::<RequestPayload>(request.payload.clone()) {
        Ok(payload) => match payload {
            RequestPayload::Execute(exec_request) => {
                // Validate the execution request before processing
                let validation_errors = exec_request.validate();
                if !validation_errors.is_empty() {
                    let error_messages: Vec<String> =
                        validation_errors.iter().map(ToString::to_string).collect();
                    let message = if error_messages.len() == 1 {
                        error_messages.into_iter().next().unwrap_or_default()
                    } else {
                        format!("Validation errors: {}", error_messages.join("; "))
                    };

                    error!(errors=?validation_errors, "Request validation failed");

                    return PrimitiveResponse::new(
                        request.jsonrpc,
                        request.id,
                        ResponseBody::Error(ResponseBodyError::ServerError(
                            ServerResponseError::ParseError(message),
                        )),
                    )
                    .into();
                }

                // Promote the parsed identifiers onto the request span so they
                // appear on every subsequent log for this request.
                let span = tracing::Span::current();
                span.record("context_id", field::display(&exec_request.context_id));
                span.record("method", field::display(&exec_request.method));

                info!(args=%exec_request.args_json, "Received execution request");

                let result = exec_request.handle(state).await.to_res_body();

                match &result {
                    ResponseBody::Error(err) => {
                        error!(?err, "Request failed");
                    }
                    ResponseBody::Result(_) => {
                        info!("Request completed successfully");
                    }
                }

                result
            }
            RequestPayload::SyncStatus(status_request) => {
                let span = tracing::Span::current();
                span.record("context_id", field::display(&status_request.context_id));
                span.record("method", "sync_status");

                status_request.handle(state).await.to_res_body()
            }
        },
        Err(err) => {
            error!(%err, payload=%request.payload, "Failed to parse request payload");

            ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::ParseError(err.to_string()),
            ))
        }
    };

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

        error!(%err, "Internal server error");
        ResponseBody::Error(ResponseBodyError::ServerError(
            ServerResponseError::InternalError { err: None },
        ))
    }
}
