use actix::fut::LocalBoxActorFuture;
use actix::{ActorFutureExt, ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::TrialUpgradeRequest;
use calimero_governance_store::find_local_signing_identity;
use either::Either;

use crate::handlers::update_application::{trial_migration, TrialRungs};
use crate::ContextManager;

impl Handler<TrialUpgradeRequest> for ContextManager {
    type Result = ActorResponse<Self, <TrialUpgradeRequest as Message>::Result>;

    fn handle(
        &mut self,
        TrialUpgradeRequest { context_id, rungs }: TrialUpgradeRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let context = match self.get_or_fetch_context(&context_id) {
            Ok(Some(context)) => context.meta.clone(),
            Ok(None) => return ActorResponse::reply(Ok(false)),
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        // No state to migrate, or no key to run as: this node does not hold the context.
        if *context.root_hash == [0; 32] {
            return ActorResponse::reply(Ok(false));
        }
        let executor = match find_local_signing_identity(&self.datastore, &context_id) {
            Ok(Some(executor)) => executor,
            Ok(None) => return ActorResponse::reply(Ok(false)),
            Err(err) => return ActorResponse::reply(Err(err)),
        };
        let Some(lock) = self.contexts.get(&context_id).map(|c| c.lock()) else {
            return ActorResponse::reply(Ok(false));
        };

        let mut modules: LocalBoxActorFuture<Self, eyre::Result<TrialRungs>> =
            actix::fut::ready(Ok(Vec::new()))
                .into_actor(self)
                .boxed_local();
        for rung in rungs {
            let service_name = context.service_name.clone();
            modules = modules
                .then(move |loaded, act, _ctx| {
                    act.get_module_for_blob(rung.bytecode_id.into(), service_name)
                        .map(move |module, _act, _ctx| {
                            let mut loaded = loaded?;
                            loaded.push((module?, rung.migration));
                            Ok(loaded)
                        })
                        .boxed_local()
                })
                .boxed_local();
        }

        let trial = modules.then(move |loaded, act, _ctx| {
            let datastore = act.datastore.clone();
            let node_client = act.node_client.clone();
            let scope_projections = std::sync::Arc::clone(&act.scope_projections);
            async move {
                let _guard = match lock {
                    Either::Left(guard) => guard,
                    Either::Right(task) => task.await,
                };
                trial_migration(
                    &datastore,
                    &scope_projections,
                    node_client,
                    &context,
                    executor,
                    loaded?,
                )
                .await?;
                Ok(true)
            }
            .into_actor(act)
        });

        ActorResponse::r#async(trial)
    }
}
