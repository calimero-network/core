use actix::{ActorResponse, ActorTryFutureExt, Handler, Message, WrapFuture};
use calimero_context_primitives::client::ContextClient;
use calimero_context_primitives::messages::update_application::UpdateApplicationRequest;
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
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
        if let Some(context) = self.contexts.get(&context_id) {
            if application_id == context.meta.application_id {
                return ActorResponse::reply(Ok(()));
            }
        }

        ActorResponse::r#async(Box::pin(
            update_application_id(
                self.node_client.clone(),
                self.context_client.clone(),
                context_id,
                application_id,
                public_key,
            )
            .into_actor(self)
            .and_then(move |blob_id, act, _ctx| {
                if let Some(context) = act.contexts.get_mut(&context_id) {
                    context.blob = blob_id;
                    context.meta.application_id = application_id;
                }

                async move { Ok(()) }.into_actor(act)
            }),
        ))
    }
}

pub async fn update_application_id(
    node_client: NodeClient,
    context_client: ContextClient,
    context_id: ContextId,
    application_id: ApplicationId,
    public_key: PublicKey,
) -> eyre::Result<BlobId> {
    let Some(application) = node_client.get_application(&application_id)? else {
        bail!("application with id '{}' not found", application_id);
    };

    if !node_client.has_blob(&application.blob)? {
        bail!("application with id '{}' has no blob", application_id);
    }

    let Some(config_client) = context_client.context_config(&context_id)? else {
        bail!("context '{}' does not exist", context_id);
    };

    let external_client = context_client.external_client(&context_id, &config_client)?;

    let blob_id = application.blob;

    external_client
        .config()
        .update_application(&public_key, &application)
        .await?;

    context_client
        .update_application(&context_id, &application_id, &public_key)
        .await?;

    Ok(blob_id)
}
