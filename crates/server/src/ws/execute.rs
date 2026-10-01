//! WebSocket `execute` (query/mutate) request handling.
//!
//! This is the bidirectional counterpart to the unidirectional event streaming
//! (`subscribe`/`unsubscribe`): clients can issue context method calls over the
//! same socket and receive the result back. The execution itself is shared with
//! the JSON-RPC server via [`crate::execute::execute_request`], and the wire
//! envelope reuses the JSON-RPC [`ExecutionRequest`] so validation and shape
//! match `/jsonrpc` exactly.
//!
//! The upgrade is gated on `context:subscribe` alone, so each `execute` is
//! checked here against the permissions the connection's token carried.

use calimero_primitives::context::ContextId;
use calimero_server_primitives::jsonrpc::ExecutionRequest;
use calimero_server_primitives::validation::Validate;
use calimero_server_primitives::ws::{ResponseBody, ResponseBodyError, ServerResponseError};
use mero_auth::auth::permissions::{
    ContextPermission, Permission, PermissionValidator, ResourceScope, UserScope,
};
use tracing::{error, field, info, warn, Span};

use crate::auth::GrantedPermissions;
use crate::caller_account::EventCaller;
use crate::execute::{execute_request, CallerIdentity};
use crate::ws::ServiceState;

/// Whether the connection's token lets it call `method` in `context_id`. With no
/// token on the socket, only a node that runs no auth serves it.
fn may_execute(
    auth_enabled: bool,
    granted: Option<&GrantedPermissions>,
    context_id: &ContextId,
    method: &str,
) -> bool {
    let Some(granted) = granted else {
        return !auth_enabled;
    };
    let required = [Permission::Context(ContextPermission::Execute(
        ResourceScope::Specific(vec![context_id.to_string()]),
        UserScope::Any,
        Some(method.to_owned()),
    ))];
    PermissionValidator::new().validate_permissions(&granted.0, &required)
}

/// Validate and run an `execute` request, producing the response body to send
/// back over the socket. Mirrors the JSON-RPC handler's mapping: validation
/// failures become `ParseError`s, handler failures become `HandlerError`s, and
/// serialization failures become `InternalError`s.
pub(crate) async fn handle(
    state: &ServiceState,
    caller: Option<EventCaller>,
    node_owner: bool,
    granted: Option<&GrantedPermissions>,
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
        Some(EventCaller::Key(key)) => CallerIdentity::Key(key),
        // An account-anchored session can now hold a WebSocket (#3942), which it
        // could not before, so this arm is reachable where it was not. It is
        // refused rather than served: `execute` runs as a context identity, and
        // an account that runs no node holds none. A delegated device's write
        // goes through `POST /admin-api/contexts/:id/intents` with a warrant,
        // which is what carries the author's consent; a session alone is not
        // that consent and must not be spent as if it were.
        Some(EventCaller::Account(account)) => {
            warn!(
                %account,
                "refusing WS execute for an account-anchored session: a delegated write \
                 needs a warrant via POST /admin-api/contexts/:id/intents"
            );
            return ResponseBody::Error(ResponseBodyError::ServerError(
                ServerResponseError::ParseError(
                    "an account-authenticated session cannot execute directly; \
                     submit a warranted intent instead"
                        .to_owned(),
                ),
            ));
        }
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
    if !may_execute(
        state.auth_enabled,
        granted,
        &request.context_id,
        &request.method,
    ) {
        warn!("refusing WS execute: token lacks context:execute");
        return ResponseBody::Error(ResponseBodyError::ServerError(
            ServerResponseError::ParseError(
                "this connection's token does not grant context:execute for this call".to_owned(),
            ),
        ));
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_primitives::context::ContextId;

    use super::may_execute;
    use crate::auth::GrantedPermissions;

    fn granted(permissions: &[&str]) -> GrantedPermissions {
        GrantedPermissions(
            permissions
                .iter()
                .map(|p| (*p).to_owned())
                .collect::<Arc<[_]>>(),
        )
    }

    #[test]
    fn a_grant_scoped_to_one_context_covers_only_that_context() {
        let ctx = ContextId::from([1u8; 32]);
        let other = ContextId::from([2u8; 32]);
        let scoped = granted(&[&format!("context:execute[{ctx}]")]);

        assert!(may_execute(true, Some(&scoped), &ctx, "set"));
        assert!(!may_execute(true, Some(&scoped), &other, "set"));
    }

    #[test]
    fn a_method_scoped_grant_covers_only_that_method() {
        let ctx = ContextId::from([1u8; 32]);
        let scoped = granted(&[&format!("context:execute[{ctx},,set]")]);

        assert!(may_execute(true, Some(&scoped), &ctx, "set"));
        assert!(!may_execute(true, Some(&scoped), &ctx, "delete"));
    }

    #[test]
    fn subscribe_alone_and_admin_and_global_execute() {
        let ctx = ContextId::from([1u8; 32]);

        assert!(!may_execute(
            true,
            Some(&granted(&["context:subscribe"])),
            &ctx,
            "set"
        ));
        assert!(may_execute(
            true,
            Some(&granted(&["context:execute"])),
            &ctx,
            "set"
        ));
        assert!(may_execute(true, Some(&granted(&["admin"])), &ctx, "set"));
    }

    #[test]
    fn no_token_on_the_socket_is_served_only_when_the_node_runs_no_auth() {
        let ctx = ContextId::from([1u8; 32]);

        assert!(!may_execute(true, None, &ctx, "set"));
        assert!(may_execute(false, None, &ctx, "set"));
    }
}
