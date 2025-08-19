use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::update_application::UpdateApplicationRequest;
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
        let context = self.contexts.get(&context_id).map(|c| c.meta);

        if let Some(context) = context {
            if application_id == context.application_id {
                return ActorResponse::reply(Ok(()));
            }
        }

        let application = self.applications.get(&application_id).cloned();

        let task = update_application_id(
            self.datastore.clone(),
            self.node_client.clone(),
            self.context_client.clone(),
            context_id,
            context,
            application_id,
            application,
            public_key,
        );

        ActorResponse::r#async(task.into_actor(self).map_ok(move |application, act, _ctx| {
            let _ignored = act
                .applications
                .entry(application_id)
                .or_insert(application);

            if let Some(context) = act.contexts.get_mut(&context_id) {
                context.meta.application_id = application_id;
            }
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
        ),
    )?;

    node_client.sync(Some(&context_id), None).await?;

    Ok(application)
}
