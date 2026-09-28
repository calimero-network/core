use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{
    GroupMemberEntry, ListGroupMembersRequest, ListGroupMembersResponse,
};
use calimero_governance_store::{MembershipRepository, MetadataRepository};

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
            let (node_identity, _) = self.require_group_signing_key(&group_id)?;
            // Fold the ephemeral projection ONCE for this request: the membership
            // gate below and the effective-member enumeration further down both read
            // from it (one RocksDB DAG walk, not two). `None` (store fault) falls
            // back to live for both.
            let proj_ctx = crate::scope_projection::ScopeProjections::ephemeral_projection(
                &self.datastore,
                &group_id,
            );
            let is_member = match &proj_ctx {
                Some((proj, _ns, heads)) => {
                    proj.member_now_checked_with(&self.datastore, &group_id, &node_identity, heads)?
                }
                None => {
                    let node_account =
                        crate::member_account::require(&self.datastore, &group_id, &node_identity)?;
                    MembershipRepository::new(&self.datastore)
                        .is_member(&group_id, &node_account)?
                }
            };
            if !is_member {
                // Typed so the admin API surfaces this precondition as a 403,
                // not a generic 500 (see `parse_api_error`).
                return Err(crate::error::ContextError::NotAGroupMember {
                    group_id: format!("{group_id:?}"),
                }
                .into());
            }

            // Effective membership = stored explicit rows ∪ inherited
            // members of `Open` subgroups. `list_group_members` alone
            // returns only the stored rows; a peer who joined via
            // `join_subgroup_inheritance` has no row (the apply path
            // `execute_member_joined_open` is validate-only), so without
            // the union they would never surface here even though
            // `check_group_membership` reports them as members (#2371).
            //
            // From the PROJECTION's effective-member enumeration with roles
            // (`member_entries_with`): the identity set validated divergence-free on
            // the `membership-enum` plane, the roles on `membership-role`. `None`
            // falls back to the live `list ∪ enumerate_inherited` union, which already
            // carries roles. The `None` reasons are an empty/unfed namespace, a cited
            // ancestry not fully folded, a target group whose direct membership isn't
            // folded (materialized fallback) — all expected steady-state deferrals —
            // and a fold inconsistency, the one abnormal case, already logged with a
            // `unified_projection_divergence` warn inside `member_entries_with`. The
            // live fallback retires in #29b.
            //
            // Pagination must span the union, so the full set is collected here and
            // sliced after the merge rather than pushed down into the store query.
            // Known cost: a paginated call is O(total effective members), not
            // O(offset+limit). This is acceptable — group membership is
            // governance-scale (a permission hierarchy), not an unbounded user list.
            // The inherited set is disjoint from the stored rows by construction
            // (`enumerate_inherited` excludes direct members of `group_id`), so no
            // dedup is needed.
            let mut members = proj_ctx
                .as_ref()
                .and_then(|(proj, ns, heads)| {
                    proj.member_entries_with(&self.datastore, *ns, &group_id, heads)
                })
                .map_or_else(
                    || -> eyre::Result<Vec<_>> {
                        let membership = MembershipRepository::new(&self.datastore);
                        let mut live = membership.list(&group_id, 0, usize::MAX)?;
                        live.extend(membership.enumerate_inherited(&group_id)?);
                        Ok(live)
                    },
                    Ok,
                )?;
            // Sort by identity for a STABLE, path-independent pagination order: the
            // projection path yields `PublicKey`-sorted ids (a `BTreeSet`) while the
            // live fallback yields store order, so without this a request served by
            // the projection and a later page served by the live fallback (e.g. mid-
            // backfill) could skip or repeat members across pages. Sorting both paths
            // the same way makes `skip(offset).take(limit)` consistent regardless of
            // which produced the set.
            members.sort_by_key(|(identity, _)| *identity);

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

            Ok(ListGroupMembersResponse { members: entries })
        })();

        ActorResponse::reply(result)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use calimero_governance_store::MetaRepository;
    use calimero_primitives::application::ApplicationId;
    use calimero_store::db::InMemoryDB;
    use calimero_store::key::{GroupMetaValue, GroupTarget};
    use calimero_store::Store;

    use super::*;
    use crate::error::ContextError;
    use crate::test_support::actor;

    const GROUP: [u8; 32] = [0xA7; 32];

    async fn list(harness: &actor::Harness) -> eyre::Report {
        harness
            .manager
            .send(ListGroupMembersRequest {
                group_id: GROUP.into(),
                offset: 0,
                limit: 10,
            })
            .await
            .expect("the manager answers")
            .expect_err("this node holds no identity in the group")
    }

    /// A group this node has never heard of is absent (404), and one it holds
    /// but takes no part in is a refusal (403). Both used to be the untyped
    /// "node has no group identity configured", which answered 500.
    #[actix::test]
    async fn a_missing_identity_says_whether_the_group_exists() {
        let store = Store::new(Arc::new(InMemoryDB::owned()));
        let harness = actor::over(store.clone()).await;

        let err = list(&harness).await;
        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::GroupNotFound { .. })
            ),
            "an unknown group should read as absent; got: {err:#}"
        );

        MetaRepository::new(&store)
            .save(
                &GROUP.into(),
                &GroupMetaValue {
                    target: GroupTarget {
                        application_id: ApplicationId::from([0xCC; 32]),
                        bytecode_id: [0xBB; 32],
                        package: Box::default(),
                        version: Box::default(),
                    },
                    created_at: 1_700_000_000,
                    admin_identity: calimero_account::AccountId::from([0x01; 32]),
                    owner_identity: calimero_account::AccountId::from([0x01; 32]),
                    migration: None,
                    auto_join: true,
                },
            )
            .expect("hold the group");

        let err = list(&harness).await;
        assert!(
            matches!(
                err.downcast_ref::<ContextError>(),
                Some(ContextError::NotAGroupMember { .. })
            ),
            "a held group this node is not in should read as a refusal; got: {err:#}"
        );
    }
}
