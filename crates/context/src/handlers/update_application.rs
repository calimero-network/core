use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::UpdateApplicationRequest;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::PublicKey;
use calimero_store::{key, types};
use eyre::bail;

use crate::ContextManager;

impl Handler<UpdateApplicationRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateApplicationRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateApplicationRequest {
            context_id,
            application_id,
            public_key,
        }: UpdateApplicationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let mut context_meta = self.repository.peek(&context_id).map(|c| c.meta.clone());

        if let Some(ref context) = context_meta {
            if application_id == context.application_id {
                return ActorResponse::reply(Ok(()));
            }
        } else {
            let context = match self.context_client().get_context(&context_id) {
                Ok(Some(ctx)) => ctx,
                Ok(None) => return ActorResponse::reply(Err(eyre::eyre!("context '{}' does not exist", context_id))),
                Err(err) => return ActorResponse::reply(Err(err)),
            };

            context_meta = Some(context);
        }

        let application = self.app_manager.get_application(&application_id)
            .ok()
            .flatten()
            .cloned();

        let task = update_application_id(
            self.datastore.clone(),
            self.node_client.clone(),
            self.context_client().clone(),
            context_id,
            context_meta,
            application_id,
            application,
            public_key,
        );

        ActorResponse::r#async(task.into_actor(self).map_ok(move |application, act, _ctx| {
            // Cache the application
            act.app_manager.put_application(application_id, application);

            // Update application ID in cache via repository
            let _updated = act.repository.update_application_id(&context_id, application_id);
        }))
    }
}

pub async fn update_application_id(
    datastore: calimero_store::Store,
    node_client: NodeClient,
    context_client: ContextClient,
    context_id: ContextId,
    context: Option<Context>,
    application_id: ApplicationId,
    application: Option<Application>,
    public_key: PublicKey,
) -> eyre::Result<Application> {
    let context = match context {
        Some(context) => context,
        None => {
            let Some(context) = context_client.get_context(&context_id)? else {
                bail!("context '{}' does not exist", context_id);
            };

            context
        }
    };

    let application = match application {
        Some(application) => application,
        None => {
            let Some(application) = node_client.get_application(&application_id)? else {
                bail!("application with id '{}' not found", application_id);
            };

            application
        }
    };

    let Some(config_client) = context_client.context_config(&context_id)? else {
        bail!(
            "missing context config parameters for context '{}'",
            context_id
        );
    };

    let external_client = context_client.external_client(&context_id, &config_client)?;

    external_client
        .config()
        .update_application(&public_key, &application)
        .await?;

    let mut handle = datastore.handle();

    handle.put(
        &key::ContextMeta::new(context.id),
        &types::ContextMeta::new(
            key::ApplicationMeta::new(application.id),
            *context.root_hash,
            context.dag_heads.clone(),
        ),
    )?;

    node_client.sync(Some(&context_id), None).await?;

    Ok(application)
}
