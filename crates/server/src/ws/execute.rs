//! WebSocket `execute` (query/mutate) request handling.
//!
//! This is the bidirectional counterpart to the unidirectional event streaming
//! (`subscribe`/`unsubscribe`): clients can issue context method calls over the
//! same socket and receive the result back. The execution itself is shared with
//! the JSON-RPC server via [`crate::execute::execute_request`], and the wire
//! envelope reuses the JSON-RPC [`ExecutionRequest`] so validation and shape
//! match `/jsonrpc` exactly.
//!
//! Auth is connection-level: the HTTP upgrade is gated by `auth::guard_layer`
//! (see `service_mounts`), and individual messages are not re-authenticated, so
//! `execute` here runs with the same authority as the connection's subscriptions.

use calimero_server_primitives::jsonrpc::ExecutionRequest;
use calimero_server_primitives::validation::Validate;
use calimero_server_primitives::ws::{ResponseBody, ResponseBodyError, ServerResponseError};
use tracing::{error, info};

use crate::execute::execute_request;
use crate::ws::ServiceState;

/// Validate and run an `execute` request, producing the response body to send
/// back over the socket. Mirrors the JSON-RPC handler's mapping: validation
/// failures become `ParseError`s, handler failures become `HandlerError`s, and
/// serialization failures become `InternalError`s.
pub(crate) async fn handle(state: &ServiceState, request: ExecutionRequest) -> ResponseBody {
    let validation_errors = request.validate();
    if !validation_errors.is_empty() {
        let error_messages: Vec<String> =
            validation_errors.iter().map(ToString::to_string).collect();
        let message = if error_messages.len() == 1 {
            error_messages.into_iter().next().unwrap_or_default()
        } else {
            format!("Validation errors: {}", error_messages.join("; "))
        };

        error!(errors = ?validation_errors, "Request validation failed");

        return ResponseBody::Error(ResponseBodyError::ServerError(
            ServerResponseError::ParseError(message),
        ));
    }

    let context_id = request.context_id;
    let method = request.method.clone();

    info!(%context_id, %method, "Received execution request");

    match execute_request(&state.ctx_client, request).await {
        Ok(response) => match serde_json::to_value(response) {
            Ok(value) => {
                info!(%context_id, %method, "Request completed successfully");
                ResponseBody::Result(value)
            }
            Err(err) => internal_error(err),
        },
        Err(err) => {
            error!(%context_id, %method, ?err, "Request failed");
            match serde_json::to_value(err) {
                Ok(value) => ResponseBody::Error(ResponseBodyError::HandlerError(value)),
                Err(err) => internal_error(err),
            }
        }
    }
}

fn internal_error(err: serde_json::Error) -> ResponseBody {
    ResponseBody::Error(ResponseBodyError::ServerError(
        ServerResponseError::InternalError {
            err: Some(err.into()),
        },
    ))
}
