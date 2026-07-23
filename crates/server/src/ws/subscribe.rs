use std::sync::Arc;

use calimero_context::group_store::MembershipRepository;
use calimero_context_config::types::ContextGroupId;
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
    // Snapshot the connection's identity under a short read lock. The membership
    // lookups below can touch the store, so we must not hold a lock across them:
    // holding the write lock across `has_member` would stall the node-event task
    // that reads `subscriptions` on every broadcast.
    let (caller, node_owner) = {
        let inner = connection_state.inner.read().await;
        (inner.caller, inner.node_owner)
    };

    // Only subscribe to contexts this connection is authorized to observe.
    // Context events carry state roots and application execution-event payloads,
    // so delivering them to a non-member is a cross-context data leak. The node
    // owner (and a no-auth dev server) may observe everything; any other
    // connection must prove membership via its authenticated caller identity.
    // Unauthorized ids are dropped rather than subscribed, and the response
    // reflects only the contexts that were actually subscribed. This runs
    // without holding any lock.
    let mut subscribed = Vec::with_capacity(request.context_ids.len());
    for id in request.context_ids {
        let caller_is_member =
            caller.map(|key| {
                state.ctx_client.has_member(&id, &key).unwrap_or_else(|err| {
                warn!(context_id=%id, %err, "has_member lookup failed; denying subscription");
                false
            })
            });

        if may_observe_context(state.auth_enabled, node_owner, caller_is_member) {
            subscribed.push(id);
        } else {
            warn!(context_id=%id, "denying WS subscription: caller is not a member of the context");
        }
    }

    // Authorize by effective (deny-list-aware) group membership, not is_member:
    // a kicked inherited member keeps a path but is denied, and must not observe.
    // Subscribe-time only, like may_observe_context.
    let mut subscribed_groups = Vec::with_capacity(request.group_ids.len());
    for group_id in request.group_ids {
        let caller_is_member = caller.map(|key| {
            let gid = ContextGroupId::from(*group_id.as_bytes());
            MembershipRepository::new(state.ctx_client.datastore())
                .effective_capabilities(&gid, &key)
                .map(|caps| caps.is_some())
                .unwrap_or_else(|err| {
                    warn!(group_id=%group_id, %err, "group effective-membership lookup failed; denying subscription");
                    false
                })
        });

        if may_observe_group(state.auth_enabled, node_owner, caller_is_member) {
            subscribed_groups.push(group_id);
        } else {
            warn!(group_id=%group_id, "denying WS group subscription: caller is not a member of the group");
        }
    }

    // Acquire the write lock only to record the approved subscriptions.
    {
        let mut inner = connection_state.inner.write().await;
        for id in &subscribed {
            let _ = inner.subscriptions.insert(*id);
        }
        for gid in &subscribed_groups {
            let _ = inner.group_subscriptions.insert(*gid);
        }
    }

    Ok(SubscribeResponse {
        context_ids: subscribed,
        group_ids: subscribed_groups,
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

/// Whether a connection may subscribe to a group's membership events. Identical
/// gate shape to [`may_observe_context`], but requires effective (deny-list-aware) membership.
pub(crate) fn may_observe_group(
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
    use super::{may_observe_context, may_observe_group};

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

    #[test]
    fn group_gate_owner_and_no_auth_observe_everything() {
        assert!(may_observe_group(true, true, None));
        assert!(may_observe_group(false, false, None));
    }

    #[test]
    fn group_gate_member_allowed_others_denied() {
        assert!(may_observe_group(true, false, Some(true)));
        assert!(!may_observe_group(true, false, Some(false)));
        assert!(!may_observe_group(true, false, None));
    }
}
