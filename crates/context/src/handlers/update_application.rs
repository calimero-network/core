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
        let node_client = self.node_client.clone();

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
                let context = act.contexts.get(&context_id).cloned();

                async move {
                    if let Some(context) = context {
                        let mut context = context.lock().await;

                        if context.blob.is_some() {
                            context.blob = node_client
                                .get_application_blob(&application_id)
                                .await?
                                .map(Vec::into_boxed_slice);
                        }
                    }

                    Ok(res)
                }
                .into_actor(act)
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
        bail!("Application with id '{}' not found", application_id);
    };

    let Some(external_client) = context_client.external_client(&context_id)? else {
        bail!("failed to initialize external client for '{}'", context_id);
    };

    external_client
        .config()
        .update_application(&public_key, application)
        .await?;

    external_client.update_application_id(&context_id, &application_id, &public_key);

    Ok(())
}
