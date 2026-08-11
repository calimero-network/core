use std::collections::HashSet;
use std::sync::Arc;

use calimero_context_client::client::ContextClient;
use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::MembershipRepository;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
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
    // Subscribe-time only, like may_observe_context. Admin authority is resolved
    // in the same pass, since admin-only payloads ride the same subscription.
    let mut subscribed_groups = Vec::with_capacity(request.group_ids.len());
    let mut admin_groups = Vec::new();
    for group_id in request.group_ids {
        if caller_may_observe_group(
            &state.ctx_client,
            state.auth_enabled,
            node_owner,
            caller.as_ref(),
            &group_id,
        ) {
            if caller_may_observe_group_as_admin(
                &state.ctx_client,
                state.auth_enabled,
                node_owner,
                caller.as_ref(),
                &group_id,
            ) {
                admin_groups.push(group_id);
            }
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
        for gid in &admin_groups {
            let _ = inner.admin_group_subscriptions.insert(*gid);
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

/// Group-subscription authorization gate, shared by the WS and SSE handlers so
/// the deny-list-aware membership check lives in one place. Resolves effective
/// membership for `caller` then applies [`may_observe_group`].
pub(crate) fn caller_may_observe_group(
    ctx_client: &ContextClient,
    auth_enabled: bool,
    node_owner: bool,
    caller: Option<&PublicKey>,
    group_id: &Hash,
) -> bool {
    let caller_is_member = caller.map(|key| {
        let gid = ContextGroupId::from(*group_id.as_bytes());
        MembershipRepository::new(ctx_client.datastore())
            .effective_capabilities(&gid, key)
            .map(|caps| caps.is_some())
            .unwrap_or_else(|err| {
                warn!(group_id=%group_id, %err, "group effective-membership lookup failed; denying subscription");
                false
            })
    });
    may_observe_group(auth_enabled, node_owner, caller_is_member)
}

/// Whether a connection additionally holds ADMIN authority over `group_id`, the
/// gate for group events whose payload is admin-only.
///
/// The predicate is `is_admin` on the subscribed id itself - the same authority
/// the `migration-status` read requires. It is deliberately NOT re-keyed to the
/// descendant subgroup a payload names: for a Restricted subgroup, `check_path`
/// returns before its ancestor-admin arm, so an admin of the root resolves to
/// no membership there and would be refused its own cascade detail.
pub(crate) fn caller_may_observe_group_as_admin(
    ctx_client: &ContextClient,
    auth_enabled: bool,
    node_owner: bool,
    caller: Option<&PublicKey>,
    group_id: &Hash,
) -> bool {
    let caller_is_admin = caller.map(|key| {
        let gid = ContextGroupId::from(*group_id.as_bytes());
        MembershipRepository::new(ctx_client.datastore())
            .is_admin(&gid, key)
            .unwrap_or_else(|err| {
                warn!(group_id=%group_id, %err, "group admin lookup failed; denying admin-only detail");
                false
            })
    });
    may_observe_group(auth_enabled, node_owner, caller_is_admin)
}

/// Whether a group-keyed event may be delivered to a connection holding these
/// subscription sets. Shared by the WS fan-out and the SSE per-session task so
/// the per-variant rule cannot hold on one transport and not the other.
///
/// `admin_only` payloads ride `admin_groups` (always a subset of `groups`), so a
/// plain member subscribed to the namespace keeps the counter-only frames and
/// loses the ones naming descendant subgroups.
pub(crate) fn may_deliver_group_event(
    admin_only: bool,
    group_id: &Hash,
    groups: &HashSet<Hash>,
    admin_groups: &HashSet<Hash>,
) -> bool {
    if admin_only {
        admin_groups.contains(group_id)
    } else {
        groups.contains(group_id)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use calimero_primitives::events::GroupMigrationPayload;
    use calimero_primitives::hash::Hash;

    use super::{may_deliver_group_event, may_observe_context, may_observe_group};

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

    /// The cascade frame names a descendant subgroup id, so a plain member of
    /// the namespace must not receive it while still receiving the counter-only
    /// progress frames. Both the WS fan-out and the SSE session task decide
    /// delivery through this function, so the rule cannot diverge between them.
    #[test]
    fn cascade_detail_reaches_admins_only_while_progress_reaches_members() {
        let group = Hash::from([0x5au8; 32]);
        let subscribed: HashSet<Hash> = [group].into_iter().collect();
        let no_admin = HashSet::new();

        let progress = GroupMigrationPayload::MigrationProgress {
            migrated: 1,
            in_progress: 1,
            unknown: 0,
            failed: 0,
            total: 2,
        };
        let cascade = GroupMigrationPayload::CascadeProgress {
            subgroup_id: Hash::from([0xccu8; 32]),
            completed: 1,
            total: 2,
        };

        for (payload, member_gets, name) in
            [(&progress, true, "progress"), (&cascade, false, "cascade")]
        {
            assert_eq!(
                may_deliver_group_event(
                    payload.requires_group_admin(),
                    &group,
                    &subscribed,
                    &no_admin
                ),
                member_gets,
                "a non-admin member subscriber and the {name} frame"
            );
            assert!(
                may_deliver_group_event(
                    payload.requires_group_admin(),
                    &group,
                    &subscribed,
                    &subscribed
                ),
                "an admin subscriber must receive the {name} frame"
            );
        }
    }
}
