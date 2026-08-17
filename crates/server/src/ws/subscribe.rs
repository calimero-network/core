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

    let groups = authorize_group_subscriptions(
        &state.ctx_client,
        state.auth_enabled,
        node_owner,
        caller.as_ref(),
        request.group_ids,
    );
    for group_id in &groups.denied {
        warn!(group_id=%group_id, "denying WS group subscription: caller is not a member of the group");
    }

    // Acquire the write lock only to record the approved subscriptions.
    {
        let mut guard = connection_state.inner.write().await;
        let inner = &mut *guard;
        for id in &subscribed {
            let _ = inner.subscriptions.insert(*id);
        }
        groups.apply(
            &mut inner.group_subscriptions,
            &mut inner.admin_group_subscriptions,
        );
    }
    let subscribed_groups = groups.subscribed;

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
    //
    // # Wire contract: events for a context may precede its subscribe ack
    //
    // `mount_method!` writes the `id`-tagged ack only after this function
    // returns, while the pushes below (and any live delta) go onto the
    // connection's command channel before that. A client can therefore see
    // `Ephemeral` frames (`id: null`) for a context *before* the ack naming it.
    //
    // This is not fixable by pushing the seed after the ack, and reordering
    // would be a false reassurance: the subscription is registered above, so a
    // live delta can be pushed between that registration and the ack regardless
    // of where the seed goes. The only way to make the ack strictly first is to
    // withhold the subscription until after it — which drops exactly the deltas
    // the live-then-snapshot ordering above exists to keep.
    //
    // Clients are therefore expected to correlate on the request `id`, which is
    // unaffected, and to be able to buffer or apply `Ephemeral` frames for a
    // context they have asked for but not yet been acked for. Presence is
    // idempotent LWW state, so applying a frame "early" needs no special
    // handling. A client that instead gates on the ack before listening will
    // miss the seed and stay blank for that author until their slice next
    // *changes* — a heartbeat re-sending identical bytes produces no diff.
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

/// Whether a connection may subscribe to a group's membership events.
///
/// The same gate as [`may_observe_context`], and delegates to it so the
/// auth-bypass rule cannot drift between the two. What differs is the caller's
/// obligation, not the rule: the authority passed here is effective
/// (deny-list-aware) membership.
pub(crate) fn may_observe_group(
    auth_enabled: bool,
    node_owner: bool,
    caller_is_member: Option<bool>,
) -> bool {
    may_observe_context(auth_enabled, node_owner, caller_is_member)
}

/// One subscribe request's group decisions, as four disjoint-by-construction
/// sets: what to grant, and what to take back.
///
/// A subscribe re-authorizes every group it names, so it has to be able to
/// remove authority as well as add it. Splitting `demoted` from `denied` keeps
/// each set's meaning local: a demoted caller keeps the group and loses only the
/// admin-only payloads, a denied one loses both.
pub(crate) struct GroupSubscriptions {
    /// May observe the group. The subscribe response echoes these.
    pub(crate) subscribed: Vec<Hash>,
    /// Also holds admin authority, so the payloads naming a descendant subgroup.
    pub(crate) admin: Vec<Hash>,
    /// May observe, but holds no admin authority now - whatever an earlier
    /// subscribe granted comes off.
    demoted: Vec<Hash>,
    /// Refused outright; every authority an earlier subscribe granted comes off.
    pub(crate) denied: Vec<Hash>,
}

impl GroupSubscriptions {
    /// Fold the decisions into a connection's stored sets.
    ///
    /// Shared by both transports so a revocation cannot land on one and not the
    /// other. It matters most on SSE, whose session outlives the connection: a
    /// stale admin entry there is not bounded by reconnecting.
    pub(crate) fn apply(&self, groups: &mut HashSet<Hash>, admin_groups: &mut HashSet<Hash>) {
        for gid in &self.subscribed {
            let _ = groups.insert(*gid);
        }
        for gid in &self.admin {
            let _ = admin_groups.insert(*gid);
        }
        for gid in &self.demoted {
            let _ = admin_groups.remove(gid);
        }
        for gid in &self.denied {
            let _ = admin_groups.remove(gid);
            let _ = groups.remove(gid);
        }
    }
}

/// Authorize a subscribe request's group ids, shared by the WS and SSE handlers
/// so one transport cannot drift from the other on an authorization decision.
///
/// Authorizes by effective (deny-list-aware) group membership, not `is_member`:
/// a kicked inherited member keeps a path but is denied, and must not observe.
/// Subscribe-time only, like [`may_observe_context`]. Admin authority is
/// resolved in the same pass, since admin-only payloads ride the same
/// subscription. Reporting the denials is left to the caller, which knows how to
/// name the connection in a log line.
pub(crate) fn authorize_group_subscriptions(
    ctx_client: &ContextClient,
    auth_enabled: bool,
    node_owner: bool,
    caller: Option<&PublicKey>,
    group_ids: impl IntoIterator<Item = Hash>,
) -> GroupSubscriptions {
    let mut decided = GroupSubscriptions {
        subscribed: Vec::new(),
        admin: Vec::new(),
        demoted: Vec::new(),
        denied: Vec::new(),
    };
    for group_id in group_ids {
        let access = caller_group_access(ctx_client, auth_enabled, node_owner, caller, &group_id);
        if !access.observe {
            decided.denied.push(group_id);
            continue;
        }
        if access.admin {
            decided.admin.push(group_id);
        } else {
            decided.demoted.push(group_id);
        }
        decided.subscribed.push(group_id);
    }
    decided
}

/// What a connection may observe on one group. Named rather than a `bool` pair,
/// because the two differ by which payloads they admit and swapping them leaks.
pub(crate) struct GroupAccess {
    /// Admits the group's events, counters included.
    pub(crate) observe: bool,
    /// Additionally admits the payloads that name a descendant subgroup.
    pub(crate) admin: bool,
}

/// Group-subscription authorization gate, shared by the WS and SSE handlers so
/// the deny-list-aware membership check lives in one place. Resolves effective
/// membership and ADMIN authority for `caller` in one pass, then applies
/// [`may_observe_group`] to each.
///
/// One pass because both authorities are held by the ACCOUNT, so the caller's
/// key resolves once and both lookups hang off it; a key bound to no account
/// holds neither. `is_admin` is skipped for a non-member, whose subscription is
/// refused outright.
///
/// The admin predicate is `is_admin` on the subscribed id itself - the same
/// authority the `migration-status` read requires. It is deliberately NOT
/// re-keyed to the descendant subgroup a payload names: for a Restricted
/// subgroup, `check_path` returns before its ancestor-admin arm, so an admin of
/// the root resolves to no membership there and would be refused its own
/// cascade detail.
pub(crate) fn caller_group_access(
    ctx_client: &ContextClient,
    auth_enabled: bool,
    node_owner: bool,
    caller: Option<&PublicKey>,
    group_id: &Hash,
) -> GroupAccess {
    let resolved = caller.map(|key| {
        let gid = ContextGroupId::from(*group_id.as_bytes());
        let Some(account) = crate::caller_account::for_group(ctx_client, &gid, key) else {
            return (false, false);
        };
        let memberships = MembershipRepository::new(ctx_client.datastore());
        let member = memberships
            .effective_capabilities(&gid, &account)
            .map(|caps| caps.is_some())
            .unwrap_or_else(|err| {
                warn!(group_id=%group_id, %err, "group effective-membership lookup failed; denying subscription");
                false
            });
        if !member {
            return (false, false);
        }
        let admin = memberships.is_admin(&gid, &account).unwrap_or_else(|err| {
            warn!(group_id=%group_id, %err, "group admin lookup failed; denying admin-only detail");
            false
        });
        (member, admin)
    });
    GroupAccess {
        observe: may_observe_group(auth_enabled, node_owner, resolved.map(|(m, _)| m)),
        admin: may_observe_group(auth_enabled, node_owner, resolved.map(|(_, a)| a)),
    }
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

    /// A subscribe re-authorizes every group it names, so `apply` has to be able
    /// to take authority back, not just grant it. This matters most on SSE, whose
    /// session outlives the connection: a stale admin entry there is not bounded
    /// by reconnecting.
    #[test]
    fn apply_grants_then_revokes_on_demote_and_deny() {
        let demoted_id = Hash::from([0x11u8; 32]);
        let denied_id = Hash::from([0x22u8; 32]);
        let kept_id = Hash::from([0x33u8; 32]);

        // Both start fully authorized, the way an earlier subscribe left them.
        let mut groups: HashSet<Hash> = [demoted_id, denied_id, kept_id].into_iter().collect();
        let mut admin_groups = groups.clone();

        super::GroupSubscriptions {
            subscribed: vec![demoted_id, kept_id],
            admin: vec![kept_id],
            demoted: vec![demoted_id],
            denied: vec![denied_id],
        }
        .apply(&mut groups, &mut admin_groups);

        assert!(
            groups.contains(&demoted_id) && !admin_groups.contains(&demoted_id),
            "a demoted caller keeps the group and loses only the admin-only payloads"
        );
        assert!(
            !groups.contains(&denied_id) && !admin_groups.contains(&denied_id),
            "a denied caller loses both authorities"
        );
        assert!(
            groups.contains(&kept_id) && admin_groups.contains(&kept_id),
            "a still-authorized admin keeps both"
        );
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
            local_contexts_swapped: 1,
            local_contexts_total: 2,
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
