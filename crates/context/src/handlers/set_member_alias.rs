use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::SetMemberAliasRequest;
use calimero_context_client::local_governance::GroupOp;

use crate::ContextManager;

impl Handler<SetMemberAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberAliasRequest {
            group_id,
            member,
            alias,
            requester,
        }: SetMemberAliasRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve the requester identity to check alias ownership. Prefer the
        // namespace identity, then fall back to the group's admin identity
        // for orphaned groups (see issue #2174). The subsequent ownership
        // check (`resolved_requester != member`) still enforces that a member
        // can only set their own alias.
        let resolved_requester = match requester {
            Some(pk) => pk,
            None => match self
                .node_namespace_identity(&group_id)
                .or_else(|| self.node_group_admin_identity(&group_id))
            {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "cannot resolve node identity for group {group_id:?}: no requester \
                         provided, no namespace identity reachable (group may be orphaned — \
                         try nest_group first), and node holds no admin signing key for this group"
                    )));
                }
            },
        };

        if resolved_requester != member {
            return ActorResponse::reply(Err(eyre::eyre!("members may only set their own alias")));
        }

        // sign_and_publish_group_op calls governance_preflight once with the
        // already-resolved requester, so no double preflight.
        self.sign_and_publish_group_op(
            &group_id,
            Some(resolved_requester),
            false,
            GroupOp::MemberAliasSet { member, alias },
        )
    }
}
