use actix::{ActorTryFutureExt, Handler, Message, ResponseActFuture, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::update_application::UpdateApplicationRequest;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use eyre::bail;

use crate::ContextManager;

impl Handler<UpdateApplicationRequest> for ContextManager {
    type Result = ResponseActFuture<Self, <UpdateApplicationRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateApplicationRequest {
            context_id,
            application_id,
            public_key,
        }: UpdateApplicationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        Box::pin(
            update_application_id(
                self.node_client.clone(),
                self.context_client.clone(),
                context_id,
                application_id,
                public_key,
            )
            .into_actor(self)
            .and_then(move |res, act, _ctx| {
                if let Some(context) = act.contexts.get_mut(&context_id) {
                    context.application_id = application_id;
                }

                async move { Ok(res) }.into_actor(act)
            }),
        )
    }
}

pub async fn update_application_id(
    node_client: NodeClient,
    context_client: ContextClient,
    context_id: ContextId,
    application_id: ApplicationId,
    public_key: PublicKey,
) -> eyre::Result<()> {
    let Some(application) = node_client.get_application(&application_id)? else {
        bail!("application with id '{}' not found", application_id);
    };

    if !node_client.has_blob(&application.blob)? {
        bail!("application with id '{}' has no blob", application_id);
    }

    let Some(external_client) = context_client.external_client(&context_id)? else {
        bail!("failed to initialize external client for '{}'", context_id);
    };

    external_client
        .config()
        .update_application(&public_key, application)
        .await?;

    context_client
        .update_application_id(&context_id, &application_id, &public_key)
        .await?;

    Ok(())
}
