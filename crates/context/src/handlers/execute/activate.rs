use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::ActivateUpgradeRequest;
use calimero_governance_store::{get_group_for_context, UpgradesRepository};
use either::Either;

use super::upgrade_gate::{maybe_lazy_upgrade, should_block, UpgradeOutcome};
use crate::error::ContextError;
use crate::ContextManager;

impl Handler<ActivateUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <ActivateUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        ActivateUpgradeRequest {
            context_id,
            executor,
        }: ActivateUpgradeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let (application_id, root_hash) = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => (context.meta.application_id, *context.meta.root_hash),
            Ok(None) => return ActorResponse::reply(Ok(false)),
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        // An uninitialized context has no state to switch; execute refuses it on its own.
        if root_hash == [0; 32] {
            return ActorResponse::reply(Ok(false));
        }
        // The switch runs as `executor`, which execute refuses unless this node holds its key.
        match self.signing_identity(&context_id, &executor) {
            Ok(Some(_)) => {}
            Ok(None) => return ActorResponse::reply(Ok(false)),
            Err(err) => return ActorResponse::reply(Err(err)),
        }
        // Spares a call with nothing to switch the wait for the write lock; the
        // decision is made again under it.
        if self.upgrade_gated(&context_id)
            || maybe_lazy_upgrade(&self.datastore, &context_id, &application_id).is_none()
        {
            return ActorResponse::reply(Ok(false));
        }
        let Some(context) = self.contexts.get(&context_id) else {
            return ActorResponse::reply(Ok(false));
        };
        let guard = context.lock();

        let guard_task = async move {
            match guard {
                Either::Left(guard) => guard,
                Either::Right(task) => task.await,
            }
        }
        .into_actor(self);

        // Decided under the lock, so a concurrent caller that already switched
        // the context leaves nothing to do here.
        let switch = guard_task.then(move |guard, act, _ctx| {
            let pending = (!act.upgrade_gated(&context_id))
                .then(|| {
                    let current = act.contexts.get(&context_id)?.meta.application_id;
                    maybe_lazy_upgrade(&act.datastore, &context_id, &current)
                })
                .flatten();
            let run = match pending {
                Some(action) => act.run_lazy_upgrade(guard, context_id, executor, action),
                None => async move { Ok((guard, UpgradeOutcome::Unchanged)) }
                    .into_actor(act)
                    .boxed_local(),
            };
            run.map(move |result, _act, _ctx| match result?.1 {
                UpgradeOutcome::Advanced => Ok(true),
                UpgradeOutcome::Unchanged => Ok(false),
                UpgradeOutcome::Failed(err) => {
                    Err(err.wrap_err(ContextError::UpgradeSwitchFailed {
                        context_id: context_id.to_string(),
                    }))
                }
            })
        });

        ActorResponse::r#async(switch)
    }
}

impl ContextManager {
    /// Whether the context's group holds the in-progress write freeze, under
    /// which execute runs no lazy upgrade either.
    fn upgrade_gated(&self, context_id: &calimero_primitives::context::ContextId) -> bool {
        let Ok(Some(group_id)) = get_group_for_context(&self.datastore, context_id) else {
            return false;
        };
        matches!(
            UpgradesRepository::new(&self.datastore).load(&group_id),
            Ok(Some(upgrade)) if should_block(self.config.migration_v2, &upgrade.status)
        )
    }
}
