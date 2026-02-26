use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorFutureExt, ActorResponse, AsyncContext, Handler, Message, WrapFuture};
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{UpgradeGroupRequest, UpgradeGroupResponse};
use calimero_context_primitives::messages::MigrationParams;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{ContextId, UpgradePolicy};
use calimero_primitives::identity::PublicKey;
use calimero_store::key::{self, GroupUpgradeStatus, GroupUpgradeValue};
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
            upgrade_policy,
            from_version,
            to_version,
        } = preamble;

        // --- Persist InProgress BEFORE the async canary ---
        // This prevents a concurrent UpgradeGroupRequest from passing
        // validate_upgrade while the canary is still running.
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let initial_status = GroupUpgradeStatus::InProgress {
            total: total_contexts as u32,
            completed: 0,
            failed: 0,
        };

        let upgrade_value = GroupUpgradeValue {
            from_version,
            to_version,
            migration: migration.as_ref().map(|m| m.method.as_bytes().to_vec()),
            initiated_at: now,
            initiated_by: requester,
            status: initial_status.clone(),
        };

        if let Err(err) =
            group_store::save_group_upgrade(&self.datastore, &group_id, &upgrade_value)
        {
            return ActorResponse::reply(Err(err.into()));
        }

        // --- LazyOnAccess: update target and return without canary/propagator ---
        // Contexts will be upgraded individually on their next execution.
        // Launching a propagator would race with the lazy mechanism and could
        // invoke migration functions twice on the same context.
        if matches!(upgrade_policy, UpgradePolicy::LazyOnAccess) {
            let result = (|| {
                let mut meta = group_store::load_group_meta(&self.datastore, &group_id)?
                    .ok_or_else(|| eyre::eyre!("group not found"))?;
                meta.target_application_id = target_application_id;
                group_store::save_group_meta(&self.datastore, &group_id, &meta)?;

                info!(
                    ?group_id,
                    %target_application_id,
                    "LazyOnAccess upgrade target set; contexts will upgrade on next access"
                );

                Ok(UpgradeGroupResponse {
                    group_id,
                    status: initial_status.into(),
                })
            })();

            return ActorResponse::reply(result);
        }

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
                    // Clean up the InProgress record so the group can be retried
                    if let Err(cleanup_err) =
                        group_store::delete_group_upgrade(&datastore, &group_id_clone)
                    {
                        error!(
                            ?group_id,
                            ?cleanup_err,
                            "failed to clean up upgrade record after canary failure"
                        );
                    }
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
                    let mut meta = group_store::load_group_meta(&datastore, &group_id_clone)?
                        .ok_or_else(|| eyre::eyre!("group not found after canary"))?;

                    meta.target_application_id = target_application_id;
                    group_store::save_group_meta(&datastore, &group_id_clone, &meta)?;

                    // Update InProgress status (canary = 1 completed)
                    let status = GroupUpgradeStatus::InProgress {
                        total: total_contexts as u32,
                        completed: 1,
                        failed: 0,
                    };

                    update_upgrade_status(&datastore, &group_id_clone, status.clone())?;

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
                            1, // canary already upgraded
                        );
                        ctx.spawn(propagator.into_actor(act));
                    } else {
                        // Only one context (the canary) — mark completed
                        let completed_at = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_secs();
                        let completed_status = GroupUpgradeStatus::Completed { completed_at };
                        update_upgrade_status(
                            &datastore,
                            &group_id_clone,
                            completed_status.clone(),
                        )?;

                        return Ok(UpgradeGroupResponse {
                            group_id: group_id_clone,
                            status: completed_status.into(),
                        });
                    }

                    Ok(UpgradeGroupResponse {
                        group_id: group_id_clone,
                        status: status.into(),
                    })
                }
            },
        ))
    }
}

struct UpgradePreamble {
    canary_context_id: ContextId,
    total_contexts: usize,
    upgrade_policy: UpgradePolicy,
    from_version: String,
    to_version: String,
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
    let contexts = group_store::enumerate_group_contexts(datastore, group_id, 0, usize::MAX)?;
    if contexts.is_empty() {
        bail!("group has no contexts to upgrade");
    }

    // 6. Select canary (first context, deterministic order)
    let canary_context_id = contexts[0];

    // 7. Read current and target application versions from ApplicationMeta
    let handle = datastore.handle();

    let from_version = handle
        .get(&key::ContextMeta::new(canary_context_id))?
        .and_then(|ctx_meta| handle.get(&ctx_meta.application).ok().flatten())
        .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version));

    let to_version = handle
        .get(&key::ApplicationMeta::new(*target_application_id))?
        .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version));

    Ok(UpgradePreamble {
        canary_context_id,
        total_contexts: contexts.len(),
        upgrade_policy: meta.upgrade_policy.clone(),
        from_version,
        to_version,
    })
}

/// Maximum number of automatic retry rounds for failed context upgrades.
const MAX_AUTO_RETRIES: u32 = 3;

/// Base delay between retry rounds (doubles each round: 5s, 10s, 20s).
const RETRY_BASE_DELAY_SECS: u64 = 5;

pub(crate) async fn propagate_upgrade(
    context_client: calimero_context_primitives::client::ContextClient,
    datastore: calimero_store::Store,
    group_id: ContextGroupId,
    target_application_id: ApplicationId,
    requester: PublicKey,
    migration: Option<MigrationParams>,
    skip_context: ContextId,
    initial_completed: u32,
) {
    let contexts = match group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)
    {
        Ok(c) => c,
        Err(err) => {
            error!(
                ?group_id,
                ?err,
                "failed to enumerate contexts for propagation"
            );
            return;
        }
    };

    // Use actual enumerated count as the authoritative total so that
    // contexts added/removed since the upgrade started are reflected.
    let total_contexts = contexts.len();

    // Build the list of contexts to upgrade (excluding the canary)
    let mut pending: Vec<ContextId> = contexts
        .into_iter()
        .filter(|cid| *cid != skip_context)
        .collect();

    let mut completed: u32 = initial_completed;
    let mut failed: u32;
    let mut attempt: u32 = 0;

    loop {
        let mut next_pending = Vec::new();
        failed = 0;

        for context_id in &pending {
            // Skip contexts already running the target application to avoid
            // re-executing migrations on retry/recovery paths.
            match context_client.get_context(context_id) {
                Ok(Some(ctx)) if ctx.application_id == target_application_id => {
                    completed += 1;
                    debug!(
                        ?group_id,
                        %context_id,
                        "context already on target application, skipping"
                    );
                    // Persist progress
                    let status = GroupUpgradeStatus::InProgress {
                        total: total_contexts as u32,
                        completed,
                        failed,
                    };
                    if let Err(err) = update_upgrade_status(&datastore, &group_id, status) {
                        error!(?group_id, ?err, "failed to persist upgrade progress");
                    }
                    continue;
                }
                _ => {}
            }

            let migrate_method = migration.as_ref().map(|m| m.method.clone());

            match context_client
                .update_application(
                    context_id,
                    &target_application_id,
                    &requester,
                    migrate_method,
                )
                .await
            {
                Ok(()) => {
                    completed += 1;
                    debug!(
                        ?group_id,
                        %context_id,
                        completed,
                        total = total_contexts,
                        attempt,
                        "context upgraded successfully"
                    );
                }
                Err(err) => {
                    failed += 1;
                    next_pending.push(*context_id);
                    warn!(
                        ?group_id,
                        %context_id,
                        ?err,
                        failed,
                        attempt,
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

        // All succeeded — no retry needed
        if next_pending.is_empty() {
            break;
        }

        attempt += 1;

        // Exhausted retry attempts
        if attempt >= MAX_AUTO_RETRIES {
            warn!(
                ?group_id,
                failed = next_pending.len(),
                attempts = attempt,
                "exhausted auto-retry attempts, remaining failures left as InProgress"
            );
            break;
        }

        // Exponential backoff before retrying
        let delay_secs = RETRY_BASE_DELAY_SECS * (1 << (attempt - 1));
        info!(
            ?group_id,
            failed = next_pending.len(),
            attempt,
            delay_secs,
            "retrying failed context upgrades after delay"
        );
        tokio::time::sleep(std::time::Duration::from_secs(delay_secs)).await;

        // Reset failed count for next round and retry only the failures
        pending = next_pending;
    }

    // Final status
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let final_status = if failed == 0 {
        GroupUpgradeStatus::Completed { completed_at: now }
    } else {
        // Keep as InProgress with the final counts so manual retry can pick it up
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
        attempts = attempt + 1,
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
