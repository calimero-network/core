use actix::{ActorFutureExt, ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{RetryGroupUpgradeRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
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
        // Resolve requester: use provided value or fall back to node group identity
        let requester = match requester {
            Some(pk) => pk,
            None => match self.node_group_identity() {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        // Validate
        let result = (|| {
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            let upgrade = group_store::load_group_upgrade(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("no upgrade found for this group"))?;

            match upgrade.status {
                GroupUpgradeStatus::InProgress { failed, .. } if failed > 0 => {}
                GroupUpgradeStatus::InProgress { .. } => {
                    bail!("upgrade is in progress with no failures — nothing to retry");
                }
                GroupUpgradeStatus::Completed { .. } => {
                    bail!("upgrade is already completed");
                }
            };

            let meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            let migration = upgrade
                .migration
                .as_ref()
                .and_then(|bytes| String::from_utf8(bytes.clone()).ok())
                .map(|method| MigrationParams { method });

            // Use current context count rather than stored total which may be stale
            let current_total =
                group_store::count_group_contexts(&self.datastore, &group_id)? as u32;

            Ok((meta.target_application_id, migration, current_total))
        })();

        let (target_application_id, migration, current_total) = match result {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // Reject if a propagator is already running for this group (e.g.
        // still in its automatic backoff sleep). Spawning a second one would
        // cause conflicting status writes and double-counted completions.
        if self.active_propagators.contains(&group_id) {
            return ActorResponse::reply(Err(eyre::eyre!(
                "a propagator is already running for this group; wait for it to finish"
            )));
        }

        info!(
            ?group_id,
            %requester,
            "retrying group upgrade for failed contexts"
        );

        // Persist reset status BEFORE spawning the propagator so that
        // GET /upgrade/status immediately reflects the retry.
        let status = GroupUpgradeStatus::InProgress {
            total: current_total,
            completed: 0,
            failed: 0,
        };

        if let Err(err) =
            super::upgrade_group::update_upgrade_status(&self.datastore, &group_id, status.clone())
        {
            return ActorResponse::reply(Err(err));
        }

        // Re-spawn propagator (it will re-attempt all contexts; already-upgraded
        // ones should be handled gracefully by update_application)
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();

        self.active_propagators.insert(group_id);

        let propagator = super::upgrade_group::propagate_upgrade(
            context_client,
            datastore,
            group_id,
            target_application_id,
            migration,
            None, // no context to skip on retry
            0,    // retry: no canary assumption
        );

        ctx.spawn(propagator.into_actor(self).map(move |_, act, _| {
            act.active_propagators.remove(&group_id);
        }));

        ActorResponse::reply(Ok(UpgradeGroupResponse {
            group_id,
            status: status.into(),
        }))
    }
}
