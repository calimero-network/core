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

use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::jsonrpc::ExecutionRequest;
use calimero_server_primitives::validation::Validate;
use calimero_server_primitives::ws::{ResponseBody, ResponseBodyError, ServerResponseError};
use tracing::{error, field, info, warn, Span};

use crate::execute::{execute_request, CallerIdentity};
use crate::ws::ServiceState;

/// Validate and run an `execute` request, producing the response body to send
/// back over the socket. Mirrors the JSON-RPC handler's mapping: validation
/// failures become `ParseError`s, handler failures become `HandlerError`s, and
/// serialization failures become `InternalError`s.
pub(crate) async fn handle(
    state: &ServiceState,
    caller: Option<PublicKey>,
    node_owner: bool,
    request: ExecutionRequest,
) -> ResponseBody {
    let validation_errors = request.validate();
    if !validation_errors.is_empty() {
        let message = match validation_errors.as_slice() {
            [single] => single.to_string(),
            many => format!(
                "Validation errors: {}",
                many.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
        };

        error!(errors = ?validation_errors, "Request validation failed");

        return ResponseBody::Error(ResponseBodyError::ServerError(
            ServerResponseError::ParseError(message),
        ));
    }

    // Promote the parsed identifiers onto the request span so the shared
    // `execute_request`'s downstream logs carry them too (mirrors JSON-RPC).
    let span = Span::current();
    span.record("context_id", field::display(&request.context_id));
    span.record("method", field::display(&request.method));

    info!("Received execution request");

    let caller_identity = match caller.as_ref() {
        Some(key) => CallerIdentity::Key(key),
        None => {
            if !node_owner && state.auth_enabled {
                warn!("No auth extensions on WebSocket execute — auth guard may not be running");
                return ResponseBody::Error(ResponseBodyError::ServerError(
                    ServerResponseError::InternalError { err: None },
                ));
            }
            CallerIdentity::NodeOwner
        }
    };
    match execute_request(&state.ctx_client, caller_identity, request).await {
        Ok(response) => match serde_json::to_value(response) {
            Ok(value) => {
                info!("Request completed successfully");
                ResponseBody::Result(value)
            }
            Err(err) => internal_error(err),
        },
        Err(err) => {
            error!(?err, "Request failed");
            match serde_json::to_value(err) {
                Ok(value) => ResponseBody::Error(ResponseBodyError::HandlerError(value)),
                Err(err) => internal_error(err),
            }
        }
    }
}

fn internal_error(err: serde_json::Error) -> ResponseBody {
    error!(%err, "Internal server error");
    ResponseBody::Error(ResponseBodyError::ServerError(
        ServerResponseError::InternalError { err: None },
    ))
}
