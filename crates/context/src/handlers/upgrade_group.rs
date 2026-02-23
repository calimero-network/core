use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorFutureExt, ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{UpgradeGroupRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{GroupUpgradeStatus, GroupUpgradeValue};
use eyre::bail;
use tracing::{debug, error, info, warn};

use crate::{group_store, ContextManager};

impl Handler<UpgradeGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpgradeGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpgradeGroupRequest {
            group_id,
            target_application_id,
            requester,
            migration,
        }: UpgradeGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // --- Synchronous validation ---
        let preamble = match validate_upgrade(
            &self.datastore,
            &group_id,
            &target_application_id,
            &requester,
        ) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let UpgradePreamble {
            canary_context_id,
            total_contexts,
        } = preamble;

        // --- Async: run canary upgrade ---
        let context_client = self.context_client.clone();
        let datastore = self.datastore.clone();
        let migrate_method = migration.as_ref().map(|m| m.method.clone());

        let canary_task = async move {
            context_client
                .update_application(
                    &canary_context_id,
                    &target_application_id,
                    &requester,
                    migrate_method,
                )
                .await
        }
        .into_actor(self);

        let group_id_clone = group_id;
        let context_client_for_propagator = self.context_client.clone();
        let datastore_for_propagator = self.datastore.clone();

        ActorResponse::r#async(canary_task.map(
            move |canary_result, act, ctx| match canary_result {
                Err(err) => {
                    error!(
                        ?group_id,
                        canary=%canary_context_id,
                        ?err,
                        "canary upgrade failed, aborting group upgrade"
                    );
                    Err(eyre::eyre!(
                        "canary upgrade failed on context {canary_context_id}: {err}"
                    ))
                }
                Ok(()) => {
                    info!(
                        ?group_id,
                        canary=%canary_context_id,
                        "canary upgrade succeeded, proceeding with group upgrade"
                    );

                    // Update group's target_application_id
                    let mut meta =
                        group_store::load_group_meta(&datastore, &group_id_clone)?
                            .ok_or_else(|| eyre::eyre!("group not found after canary"))?;

                    meta.target_application_id = target_application_id;
                    group_store::save_group_meta(&datastore, &group_id_clone, &meta)?;

                    // Persist InProgress status (canary = 1 completed)
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let status = GroupUpgradeStatus::InProgress {
                        total: total_contexts as u32,
                        completed: 1,
                        failed: 0,
                    };

                    let upgrade_value = GroupUpgradeValue {
                        from_revision: 0,
                        to_revision: 0,
                        migration: migration
                            .as_ref()
                            .map(|m| m.method.as_bytes().to_vec()),
                        initiated_at: now,
                        initiated_by: requester,
                        status: status.clone(),
                    };

                    group_store::save_group_upgrade(
                        &datastore,
                        &group_id_clone,
                        &upgrade_value,
                    )?;

                    // Spawn propagator for remaining contexts
                    if total_contexts > 1 {
                        let propagator = propagate_upgrade(
                            context_client_for_propagator,
                            datastore_for_propagator,
                            group_id_clone,
                            target_application_id,
                            requester,
                            migration,
                            canary_context_id,
                            total_contexts,
                        );
                        ctx.spawn(propagator.into_actor(act));
                    } else {
                        // Only one context (the canary) — mark completed
                        let completed_status = GroupUpgradeStatus::Completed {
                            completed_at: now,
                        };
                        let mut completed_value = upgrade_value;
                        completed_value.status = completed_status.clone();
                        group_store::save_group_upgrade(
                            &datastore,
                            &group_id_clone,
                            &completed_value,
                        )?;

                        return Ok(UpgradeGroupResponse {
                            group_id: group_id_clone,
                            status: completed_status,
                        });
                    }

                    Ok(UpgradeGroupResponse {
                        group_id: group_id_clone,
                        status,
                    })
                }
            },
        ))
    }
}

struct UpgradePreamble {
    canary_context_id: ContextId,
    total_contexts: usize,
}

fn validate_upgrade(
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    target_application_id: &ApplicationId,
    requester: &PublicKey,
) -> eyre::Result<UpgradePreamble> {
    // 1. Group must exist
    let meta = group_store::load_group_meta(datastore, group_id)?
        .ok_or_else(|| eyre::eyre!("group not found"))?;

    // 2. Requester must be admin
    group_store::require_group_admin(datastore, group_id, requester)?;

    // 3. No active upgrade in progress
    if let Some(existing) = group_store::load_group_upgrade(datastore, group_id)? {
        if matches!(existing.status, GroupUpgradeStatus::InProgress { .. }) {
            bail!("an upgrade is already in progress for this group");
        }
    }

    // 4. Target must differ from current
    if meta.target_application_id == *target_application_id {
        bail!("group is already targeting this application");
    }

    // 5. Group must have contexts
    let contexts = group_store::enumerate_group_contexts(datastore, group_id)?;
    if contexts.is_empty() {
        bail!("group has no contexts to upgrade");
    }

    // 6. Select canary (first context, deterministic order)
    let canary_context_id = contexts[0];

    Ok(UpgradePreamble {
        canary_context_id,
        total_contexts: contexts.len(),
    })
}

pub(crate) async fn propagate_upgrade(
    context_client: calimero_context_primitives::client::ContextClient,
    datastore: calimero_store::Store,
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    requester: PublicKey,
    migration: Option<MigrationParams>,
    skip_context: ContextId,
    total_contexts: usize,
) {
    let contexts = match group_store::enumerate_group_contexts(&datastore, &group_id) {
        Ok(c) => c,
        Err(err) => {
            error!(?group_id, ?err, "failed to enumerate contexts for propagation");
            return;
        }
    };

    let mut completed: u32 = 1; // canary already done
    let mut failed: u32 = 0;

    for context_id in contexts {
        if context_id == skip_context {
            continue;
        }

        let migrate_method = migration.as_ref().map(|m| m.method.clone());

        match context_client
            .update_application(&context_id, &target_application_id, &requester, migrate_method)
            .await
        {
            Ok(()) => {
                completed += 1;
                debug!(
                    ?group_id,
                    %context_id,
                    completed,
                    total = total_contexts,
                    "context upgraded successfully"
                );
            }
            Err(err) => {
                failed += 1;
                warn!(
                    ?group_id,
                    %context_id,
                    ?err,
                    failed,
                    "context upgrade failed"
                );
            }
        }

        // Persist progress after each context
        let status = GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed,
            failed,
        };

        if let Err(err) = update_upgrade_status(&datastore, &group_id, status) {
            error!(?group_id, ?err, "failed to persist upgrade progress");
        }
    }

    // Final status
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let final_status = if failed == 0 {
        GroupUpgradeStatus::Completed {
            completed_at: now,
        }
    } else {
        // Keep as InProgress with the final counts so retry can pick it up
        GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed,
            failed,
        }
    };

    if let Err(err) = update_upgrade_status(&datastore, &group_id, final_status) {
        error!(?group_id, ?err, "failed to persist final upgrade status");
    }

    info!(
        ?group_id,
        completed,
        failed,
        total = total_contexts,
        "group upgrade propagation finished"
    );
}

fn update_upgrade_status(
    datastore: &calimero_store::Store,
    group_id: &ContextGroupId,
    status: GroupUpgradeStatus,
) -> eyre::Result<()> {
    if let Some(mut upgrade) = group_store::load_group_upgrade(datastore, group_id)? {
        upgrade.status = status;
        group_store::save_group_upgrade(datastore, group_id, &upgrade)?;
    }
    Ok(())
}
