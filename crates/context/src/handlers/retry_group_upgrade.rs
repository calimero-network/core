use actix::{ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{RetryGroupUpgradeRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
use calimero_primitives::context::ContextId;
use calimero_store::key::GroupUpgradeStatus;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<RetryGroupUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <RetryGroupUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        RetryGroupUpgradeRequest {
            group_id,
            requester,
        }: RetryGroupUpgradeRequest,
        ctx: &mut Self::Context,
    ) -> Self::Result {
        // Validate
        let result = (|| {
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            let upgrade = group_store::load_group_upgrade(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("no upgrade found for this group"))?;

            let (total, _completed, _failed) = match upgrade.status {
                GroupUpgradeStatus::InProgress {
                    total,
                    completed,
                    failed,
                } if failed > 0 => (total, completed, failed),
                GroupUpgradeStatus::InProgress { failed: 0, .. } => {
                    bail!("upgrade is in progress with no failures — nothing to retry");
                }
                GroupUpgradeStatus::Completed { .. } => {
                    bail!("upgrade is already completed");
                }
                GroupUpgradeStatus::RolledBack { .. } => {
                    bail!("upgrade has been rolled back — start a new upgrade instead");
                }
                _ => bail!("unexpected upgrade status"),
            };

            let meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            let migration = upgrade
                .migration
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                .map(|method| MigrationParams { method });

            Ok((meta.target_application_id, migration, total))
        })();

        let (target_application_id, migration, total) = match result {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        info!(
            ?group_id,
            %requester,
            "retrying group upgrade for failed contexts"
        );

        // Re-spawn propagator (it will re-attempt all contexts; already-upgraded
        // ones should be handled gracefully by update_application)
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();

        let propagator = super::upgrade_group::propagate_upgrade(
            context_client,
            datastore,
            group_id,
            target_application_id,
            requester,
            migration,
            // Sentinel: no context to skip on retry
            ContextId::from([0u8; 32]),
            total as usize,
        );

        ctx.spawn(propagator.into_actor(self));

        let status = GroupUpgradeStatus::InProgress {
            total,
            completed: 0,
            failed: 0,
        };

        ActorResponse::reply(Ok(UpgradeGroupResponse { group_id, status }))
    }
}
