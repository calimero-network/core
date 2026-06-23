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
            // Fold the ephemeral projection ONCE for this request: the gate below
            // and the enum shadow further down both read from it (one RocksDB DAG
            // walk, not two). `None` (store fault) falls back to live for the gate
            // and skips the shadow.
            let proj_ctx = crate::scope_projection::ScopeProjections::ephemeral_projection(
                &self.datastore,
                &group_id,
            );
            let is_member = match &proj_ctx {
                Some((proj, _ns, heads)) => {
                    proj.member_now_checked_with(&self.datastore, &group_id, &node_identity, heads)?
                }
                None => MembershipRepository::new(&self.datastore)
                    .is_member(&group_id, &node_identity)?,
            };
            if !is_member {
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
            // Reuses the single fold built above for the gate.
            if let Some((proj, ns, heads)) = &proj_ctx {
                let live_ids: std::collections::BTreeSet<_> =
                    members.iter().map(|(pk, _)| *pk).collect();
                proj.shadow_member_enum_with(&self.datastore, *ns, &group_id, heads, &live_ids);

                // ROLE shadow (precursor to flipping `list_group_members` onto the
                // projection, which — unlike the count/cohort consumers — returns
                // ROLES the identity-set shadow doesn't validate). Resolve every
                // live member's projected role in ONE fold, then compare; `None`
                // (not-yet-folded / projection abstains) skips.
                // A member ABSENT from the returned map = the projection has no role
                // opinion (not a member at the cut, the cut isn't fully folded, or
                // the group is unfolded → materialized fallback). The role shadow
                // skips it: member PRESENCE is owned by the identity shadow above
                // (`membership-enum`), so this plane only validates ROLES for members
                // both sides agree exist. Keyed lookup (not a positional zip) keeps
                // this correct regardless of the returned map's size.
                let member_ids: Vec<_> = members.iter().map(|(pk, _)| *pk).collect();
                let projected_roles =
                    proj.member_roles_for(&self.datastore, &group_id, &member_ids, heads);
                for (member, live_role) in &members {
                    if let Some(projected_role) = projected_roles.get(member) {
                        if projected_role != live_role {
                            tracing::warn!(
                                marker = "unified_projection_divergence",
                                plane = "membership-role",
                                group_id = ?group_id,
                                %member,
                                ?projected_role,
                                ?live_role,
                                "query-enum: projection member role differs from live"
                            );
                        }
                    }
                }
            }

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
