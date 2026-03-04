use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::DetachContextFromGroupRequest;
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<DetachContextFromGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <DetachContextFromGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        DetachContextFromGroupRequest {
            group_id,
            context_id,
            requester,
            signing_key,
        }: DetachContextFromGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            let current_group = group_store::get_group_for_context(&self.datastore, &context_id)?;
            if current_group.as_ref() != Some(&group_id) {
                bail!("context '{context_id}' does not belong to group '{group_id:?}'");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let group_client_result = signing_key.map(|sk| self.group_client(group_id, sk));

        ActorResponse::r#async(
            async move {
                if let Some(client_result) = group_client_result {
                    let mut group_client = client_result?;
                    group_client
                        .unregister_context_from_group(context_id)
                        .await?;
                }

                group_store::unregister_context_from_group(&datastore, &group_id, &context_id)?;

                Ok(())
            }
            .into_actor(self),
        )
    }
}
