use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{
    GroupMemberEntry, ListGroupMembersRequest, ListGroupMembersResponse,
};
use calimero_governance_store::{MembershipRepository, MetadataRepository};
use eyre::bail;

use crate::ContextManager;
use calimero_governance_store;

impl Handler<ListGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListGroupMembersRequest {
            group_id,
            offset,
            limit,
        }: ListGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                bail!("node has no group identity configured");
            };
            if !crate::scope_projection::ScopeProjections::member_now_checked(
                &self.datastore,
                &group_id,
                &node_identity,
            )? {
                bail!("node is not a member of group '{group_id:?}'");
            }

            // Effective membership = stored explicit rows ∪ inherited
            // members of `Open` subgroups. `list_group_members` alone
            // returns only the stored rows; a peer who joined via
            // `join_subgroup_inheritance` has no row (the apply path
            // `execute_member_joined_open` is validate-only), so without
            // the union they would never surface here even though
            // `check_group_membership` reports them as members (#2371).
            //
            // Pagination must span the union, so the full sets are
            // collected here and sliced after the merge rather than
            // pushed down into the store query. Known cost: a paginated
            // call is O(total effective members) in store reads, not
            // O(offset+limit). This is acceptable here — group
            // membership is governance-scale (a permission hierarchy),
            // not an unbounded user list — and the inherited set cannot
            // be store-paginated at all since it is computed, not
            // stored. The inherited set is disjoint from the stored
            // rows by construction (`enumerate_inherited_members`
            // excludes direct members of `group_id`), so no dedup is
            // needed.
            let mut members =
                MembershipRepository::new(&self.datastore).list(&group_id, 0, usize::MAX)?;
            members
                .extend(MembershipRepository::new(&self.datastore).enumerate_inherited(&group_id)?);

            // SHADOW: validate the projection's effective-member set against this
            // live union (logs `membership-enum` divergence; still returns live).
            let live_ids: std::collections::BTreeSet<_> =
                members.iter().map(|(pk, _)| *pk).collect();
            crate::scope_projection::ScopeProjections::shadow_check_member_enum(
                &self.datastore,
                &group_id,
                &live_ids,
            );

            let entries = members
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|(identity, role)| {
                    let name = MetadataRepository::new(&self.datastore)
                        .member_metadata(&group_id, &identity)
                        .ok()
                        .flatten()
                        .and_then(|r| r.name);
                    GroupMemberEntry {
                        identity,
                        role,
                        name,
                    }
                })
                .collect();

            Ok(ListGroupMembersResponse {
                members: entries,
                self_identity: node_identity,
            })
        })();

        ActorResponse::reply(result)
    }
}
