use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::{JoinGroupContextRequest, JoinGroupContextResponse};
use calimero_primitives::context::ContextConfigParams;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupContextRequest {
            group_id,
            context_id,
        }: JoinGroupContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Resolve joiner identity from node namespace identity.
        let (joiner_identity, _) = match self.node_namespace_identity(&group_id) {
            Some(id) => id,
            None => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "node has no namespace identity for this group; join the group first"
                )));
            }
        };

        // Group membership already verified above. All contexts in a group
        // a member has access to are joinable. Restricted access is handled
        // at the subgroup level (admin must explicitly add member to the subgroup).
        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group not found");
            }
            if !group_store::check_group_membership(&self.datastore, &group_id, &joiner_identity)? {
                bail!("identity is not a member of the group");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();
        let node_client = self.node_client.clone();
        ActorResponse::r#async(
            async move {
                // Use the group member's existing keys (reused across all contexts).
                let group_member_val =
                    group_store::get_group_member_value(&datastore, &group_id, &joiner_identity)?
                        .ok_or_else(|| eyre::eyre!("group member value not found"))?;

                let config = if !context_client.has_context(&context_id)? {
                    let app_id = group_store::load_group_meta(&datastore, &group_id)?
                        .map(|meta| meta.target_application_id);

                    Some(ContextConfigParams {
                        application_id: app_id,
                        application_revision: 0,
                        members_revision: 0,
                    })
                } else {
                    None
                };

                let _ignored = context_client
                    .sync_context_config(context_id, config)
                    .await?;

                // Write ContextIdentity from group member keys so the sync
                // key-share can find identity + keys for this context.
                {
                    let mut handle = datastore.handle();
                    handle.put(
                        &calimero_store::key::ContextIdentity::new(context_id, joiner_identity),
                        &calimero_store::types::ContextIdentity {
                            private_key: group_member_val.private_key,
                            sender_key: group_member_val.sender_key,
                        },
                    )?;
                }

                node_client.subscribe(&context_id).await?;
                node_client.sync(Some(&context_id), None).await?;

                info!(
                    ?group_id,
                    ?context_id,
                    %joiner_identity,
                    "joined context via group membership"
                );

                Ok(JoinGroupContextResponse {
                    context_id,
                    member_public_key: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}
