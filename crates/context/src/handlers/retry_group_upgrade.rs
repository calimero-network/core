use actix::{ActorFutureExt, ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_client::group::{RetryGroupUpgradeRequest, UpgradeGroupResponse};
use calimero_governance_store::{
    MembershipRepository, MetaRepository, MetadataRepository, UpgradesRepository,
};
use calimero_store::key::GroupUpgradeStatus;
use eyre::bail;
use tracing::info;

use crate::ContextManager;

impl Handler<RetryGroupUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <RetryGroupUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        RetryGroupUpgradeRequest { group_id }: RetryGroupUpgradeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let Some((signer, _)) = self.node_signing_key(&group_id) else {
            return ActorResponse::reply(Err(eyre::eyre!("node has no namespace identity")));
        };

        // Validate
        let result = (|| {
            let signer_account =
                crate::member_account::require(&self.datastore, &group_id, &signer)?;
            MembershipRepository::new(&self.datastore).require_admin(&group_id, &signer_account)?;

            let upgrade = UpgradesRepository::new(&self.datastore)
                .load(&group_id)?
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

            let meta = MetaRepository::new(&self.datastore)
                .load(&group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // Use current context count rather than stored total which may be stale
            let current_total =
                MetadataRepository::new(&self.datastore).count_contexts(&group_id)? as u32;

            Ok((meta.target.application_id, current_total))
        })();

        let (target_application_id, current_total) = match result {
            Ok(v) => v,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        // `upgrade.migration` is NOT evidence of what this upgrade needs: the
        // lazy path writes the record before it can read the target's ABI, and
        // crash recovery reaches failed > 0 without ever filling it in - which
        // is precisely the state retry accepts. Re-resolve from the ABIs.
        let node_client = self.node_client.clone();
        let datastore = self.datastore.clone();
        let resolve = async move {
            super::upgrade_group::resolve_resumed_migration(
                &node_client,
                &datastore,
                &group_id,
                &target_application_id,
            )
            .await
        }
        .into_actor(self);

        ActorResponse::r#async(resolve.map(move |migration, act, ctx| {
            let migration = migration?;

            // Reject if a propagator is already running for this group (e.g.
            // still in its automatic backoff sleep). Spawning a second one would
            // cause conflicting status writes and double-counted completions.
            // Checked here, after the resolve, so the await cannot stale it.
            if act.active_propagators.contains(&group_id) {
                bail!("a propagator is already running for this group; wait for it to finish");
            }

            info!(
                ?group_id,
                %signer,
                "retrying group upgrade for failed contexts"
            );

            // Persist reset status BEFORE spawning the propagator so that
            // GET /upgrade/status immediately reflects the retry.
            let status = GroupUpgradeStatus::InProgress {
                total: current_total,
                completed: 0,
                failed: 0,
            };

            super::upgrade_group::update_upgrade_status(&act.datastore, &group_id, status.clone())?;

            // Re-spawn propagator (it will re-attempt all contexts; already-upgraded
            // ones should be handled gracefully by update_application)
            act.active_propagators.insert(group_id);

            let propagator = super::upgrade_group::propagate_upgrade(
                act.context_client.clone(),
                act.node_client.clone(),
                act.datastore.clone(),
                group_id,
                target_application_id,
                migration,
            );

            ctx.spawn(propagator.into_actor(act).map(move |_, act, _| {
                act.active_propagators.remove(&group_id);
            }));

            Ok(UpgradeGroupResponse { group_id, status })
        }))
    }
}
