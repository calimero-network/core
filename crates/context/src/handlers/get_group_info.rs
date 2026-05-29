use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GetGroupInfoRequest, GroupInfoResponse};
use calimero_governance_store::{
    CapabilitiesRepository, MembershipRepository, MetaRepository, MetadataRepository,
    UpgradesRepository,
};
use eyre::bail;

use crate::ContextManager;
use calimero_governance_store;

impl Handler<GetGroupInfoRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupInfoRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupInfoRequest { group_id }: GetGroupInfoRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let Some(meta) = MetaRepository::new(&self.datastore).load(&group_id)? else {
                bail!("group '{group_id:?}' not found");
            };

            let Some((node_identity, _)) = self.node_namespace_identity(&group_id) else {
                bail!("node has no group identity configured");
            };
            if !MembershipRepository::new(&self.datastore).is_member(&group_id, &node_identity)? {
                bail!("node is not a member of group '{group_id:?}'");
            }

            // Effective member count = stored rows + inherited members,
            // kept consistent with the `list_group_members` handler
            // (#2371). `count_group_members` alone counts only stored
            // `GroupMember` rows, so on an Open subgroup it would
            // under-report and a client paginating `list_group_members`
            // against this count would miss inherited members on the
            // final page.
            //
            // Cost: inherited membership is derived state with no stored
            // counter, so an accurate count requires resolving it — a
            // chain walk bounded by `MAX_NAMESPACE_DEPTH`. This is minor
            // next to `compute_group_state_hash` below, which already
            // scans the full group state on every call.
            let member_count = (MembershipRepository::new(&self.datastore).count(&group_id)?
                + MembershipRepository::new(&self.datastore)
                    .enumerate_inherited(&group_id)?
                    .len()) as u64;

            let context_count =
                MetadataRepository::new(&self.datastore).count_contexts(&group_id)? as u64;

            let active_upgrade = UpgradesRepository::new(&self.datastore)
                .load(&group_id)?
                .map(Into::into);

            let default_capabilities = CapabilitiesRepository::new(&self.datastore)
                .default_capabilities(&group_id)?
                .unwrap_or(0);

            let subgroup_visibility = match CapabilitiesRepository::new(&self.datastore)
                .subgroup_visibility(&group_id)?
            {
                calimero_context_config::VisibilityMode::Open => "open".to_owned(),
                calimero_context_config::VisibilityMode::Restricted => "restricted".to_owned(),
            };

            let metadata = MetadataRepository::new(&self.datastore).group_metadata(&group_id)?;

            let state_hash = MetaRepository::new(&self.datastore).compute_state_hash(&group_id)?;

            Ok(GroupInfoResponse {
                group_id,
                app_key: meta.app_key.into(),
                target_application_id: meta.target_application_id,
                upgrade_policy: meta.upgrade_policy,
                member_count,
                context_count,
                active_upgrade,
                default_capabilities,
                subgroup_visibility,
                metadata,
                state_hash,
            })
        })();

        ActorResponse::reply(result)
    }
}
