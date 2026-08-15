use std::sync::Arc;

use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::MembershipRepository;
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
        if caller_may_observe_context(
            &state.ctx_client,
            state.auth_enabled,
            node_owner,
            caller.as_ref(),
            &id,
        ) {
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
        if caller_may_observe_group(
            &state.ctx_client,
            state.auth_enabled,
            node_owner,
            caller.as_ref(),
            &group_id,
        ) {
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

    // Seed this connection with each context's CURRENT presence, now that the
    // subscription is live.
    //
    // Order is load-bearing and deliberately this way round. Live first, then
    // snapshot: a delta landing between the two is delivered (possibly twice,
    // harmlessly — it is idempotent last-writer-wins state), whereas reading
    // the snapshot first would drop it entirely, leaving the client stale until
    // that author's slice next *changed* (a heartbeat re-sending identical
    // bytes produces no diff). The snapshot cannot be older than a delta
    // already delivered, because the node writes its awareness store before
    // emitting the diff.
    //
    // Delivery is per-connection: `try_push_event` writes to this connection's
    // own command channel, so no other subscriber sees another client's seed.
    // Every context's snapshot read is issued concurrently, so a subscribe
    // naming N contexts is bounded by ONE snapshot timeout rather than N of
    // them stacking ahead of the client's acknowledgment.
    for (_context_id, events) in
        crate::ephemeral_replay::presence_replay_many(&state.node_client, &subscribed).await
    {
        for event in events {
            connection_state.try_push_event(&event);
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

/// Context-observation authorization gate, shared by every transport that
/// hands a caller a context's live data: the WS `subscribe` handler and the
/// SSE `subscribe` handler.
///
/// Resolves the account `caller` acts as (membership rows are account-keyed)
/// and applies [`may_observe_context`]. Both the live delta stream and the
/// presence replay a subscriber is seeded with pass through this one
/// predicate, so a caller refused the stream cannot be served the seed; two
/// implementations of "may this caller see this context" could drift and let
/// one route serve what the other refuses.
///
/// Fails **closed**: a store fault during the membership lookup is warned and
/// treated as "not a member", never as an implicit grant.
pub(crate) fn caller_may_observe_context(
    ctx_client: &ContextClient,
    auth_enabled: bool,
    node_owner: bool,
    caller: Option<&calimero_primitives::identity::PublicKey>,
    context_id: &calimero_primitives::context::ContextId,
) -> bool {
    let caller_is_member = caller.map(|key| {
        let account = crate::caller_account::for_context(ctx_client, context_id, key);
        ctx_client
            .has_member(context_id, key, account)
            .unwrap_or_else(|err| {
                warn!(%context_id, %err, "has_member lookup failed; denying observation");
                false
            })
    });
    may_observe_context(auth_enabled, node_owner, caller_is_member)
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

/// Group-subscription authorization gate, shared by the WS and SSE handlers so
/// the deny-list-aware membership check lives in one place. Resolves effective
/// membership for `caller` then applies [`may_observe_group`].
pub(crate) fn caller_may_observe_group(
    ctx_client: &ContextClient,
    auth_enabled: bool,
    node_owner: bool,
    caller: Option<&calimero_primitives::identity::PublicKey>,
    group_id: &calimero_primitives::hash::Hash,
) -> bool {
    let caller_is_member = caller.map(|key| {
        let gid = ContextGroupId::from(*group_id.as_bytes());
        // Capabilities are granted to the account, so the caller's key resolves
        // first; a key bound to none holds no capability and is denied.
        let Some(account) = calimero_governance_store::member_account_in_namespace(
            ctx_client.datastore(),
            &gid,
            key,
        )
        .ok()
        .flatten() else {
            return false;
        };
        MembershipRepository::new(ctx_client.datastore())
            .effective_capabilities(&gid, &account)
            .map(|caps| caps.is_some())
            .unwrap_or_else(|err| {
                warn!(group_id=%group_id, %err, "group effective-membership lookup failed; denying subscription");
                false
            })
    });
    may_observe_group(auth_enabled, node_owner, caller_is_member)
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
