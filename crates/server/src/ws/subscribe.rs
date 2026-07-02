use std::sync::Arc;

use calimero_server_primitives::ws::{SubscribeRequest, SubscribeResponse};
use calimero_server_primitives::Infallible;
use eyre::Result as EyreResult;
use tracing::warn;

use crate::ws::{mount_method, ConnectionState, ServiceState};

mount_method!(SubscribeRequest-> Result<SubscribeResponse, Infallible>, handle);

async fn handle(
    request: SubscribeRequest,
    state: Arc<ServiceState>,
    connection_state: ConnectionState,
) -> EyreResult<SubscribeResponse> {
    let mut inner = connection_state.inner.write().await;
    let (caller, node_owner) = (inner.caller, inner.node_owner);

    // Only subscribe to contexts this connection is authorized to observe.
    // Context events carry state roots and application execution-event payloads,
    // so delivering them to a non-member is a cross-context data leak. The node
    // owner (and a no-auth dev server) may observe everything; any other
    // connection must prove membership via its authenticated caller identity.
    // Unauthorized ids are dropped rather than subscribed, and the response
    // reflects only the contexts that were actually subscribed.
    let mut subscribed = Vec::with_capacity(request.context_ids.len());
    for id in request.context_ids {
        let caller_is_member =
            caller.map(|key| state.ctx_client.has_member(&id, &key).unwrap_or(false));

        if may_observe_context(state.auth_enabled, node_owner, caller_is_member) {
            let _ = inner.subscriptions.insert(id);
            subscribed.push(id);
        } else {
            warn!(context_id=%id, "denying WS subscription: caller is not a member of the context");
        }
    }

    Ok(SubscribeResponse {
        context_ids: subscribed,
    })
}

/// Whether a connection may subscribe to (observe) a context's event stream.
///
/// The node owner and a no-auth dev server may observe everything. Any other
/// connection must present an authenticated caller that is a member of the
/// context (`caller_is_member == Some(true)`); a connection with no caller
/// identity (`None`) is denied when auth is enabled.
pub(crate) fn may_observe_context(
    auth_enabled: bool,
    node_owner: bool,
    caller_is_member: Option<bool>,
) -> bool {
    if node_owner || !auth_enabled {
        true
    } else {
        caller_is_member.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::may_observe_context;

    #[test]
    fn node_owner_observes_everything() {
        assert!(may_observe_context(true, true, None));
        assert!(may_observe_context(true, true, Some(false)));
    }

    #[test]
    fn no_auth_server_observes_everything() {
        assert!(may_observe_context(false, false, None));
    }

    #[test]
    fn member_is_allowed_non_member_and_no_caller_denied() {
        assert!(may_observe_context(true, false, Some(true)));
        assert!(!may_observe_context(true, false, Some(false)));
        assert!(!may_observe_context(true, false, None));
    }
}
