use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::{JoinContextRequest, JoinContextResponse};
use calimero_primitives::context::ContextConfigParams;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinContextRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinContextRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinContextRequest { context_id }: JoinContextRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let group_id = match group_store::get_group_for_context(&self.datastore, &context_id) {
            Ok(Some(gid)) => gid,
            Ok(None) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "context does not belong to any group"
                )));
            }
            Err(err) => {
                return ActorResponse::reply(Err(err));
            }
        };

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
                let ns_id = group_store::resolve_namespace(&datastore, &group_id)?;
                let ns_identity = group_store::get_namespace_identity(&datastore, &ns_id)?
                    .ok_or_else(|| eyre::eyre!("namespace identity not found"))?;
                let (_pk, sk_bytes, _sender) = ns_identity;

                let zero_app = calimero_primitives::application::ApplicationId::from([0u8; 32]);
                let config = if !context_client.has_context(&context_id)? {
                    let app_id = group_store::load_group_meta(&datastore, &group_id)?
                        .map(|meta| meta.target_application_id)
                        .filter(|id| *id != zero_app);

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

                {
                    let mut handle = datastore.handle();
                    handle.put(
                        &calimero_store::key::ContextIdentity::new(context_id, joiner_identity),
                        &calimero_store::types::ContextIdentity {
                            private_key: Some(sk_bytes),
                            sender_key: None,
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

                Ok(JoinContextResponse {
                    context_id,
                    member_public_key: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}
