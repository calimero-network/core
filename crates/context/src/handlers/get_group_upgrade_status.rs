use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{
    GetGroupUpgradeStatusRequest, GroupUpgradeStatusResponse, MissingRelease,
};
use calimero_governance_store::{NamespaceRepository, UpgradesRepository};
use eyre::bail;

use crate::handlers::activate_namespace_upgrade::missing_releases;
use crate::ContextManager;

impl Handler<GetGroupUpgradeStatusRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetGroupUpgradeStatusRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetGroupUpgradeStatusRequest { group_id }: GetGroupUpgradeStatusRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            let (node_identity, _) = self.require_group_signing_key(&group_id)?;
            if !crate::scope_projection::ScopeProjections::member_now_checked(
                &self.datastore,
                &group_id,
                &node_identity,
            )? {
                bail!(crate::error::ContextError::NotAGroupMember {
                    group_id: group_id.to_string(),
                });
            }
            let namespace_id = NamespaceRepository::new(&self.datastore).resolve(&group_id)?;
            let missing_releases = missing_releases(
                &self.datastore,
                |blob| self.node_client.has_blob(blob),
                &namespace_id,
            )?
            .into_iter()
            .map(|rung| MissingRelease {
                package: rung.package,
                version: rung.version,
                application_id: rung.application_id,
            })
            .collect();
            Ok(GroupUpgradeStatusResponse {
                upgrade: UpgradesRepository::new(&self.datastore)
                    .load(&group_id)?
                    .map(Into::into),
                missing_releases,
            })
        })();
        ActorResponse::reply(result)
    }
}
