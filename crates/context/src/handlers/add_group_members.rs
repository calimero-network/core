use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::group::AddGroupMembersRequest;
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<AddGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <AddGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        AddGroupMembersRequest {
            group_id,
            members,
            requester,
            signing_key,
        }: AddGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Sync validation
        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group not found");
            }
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
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
                    let signer_ids: Vec<calimero_context_config::types::SignerId> = members
                        .iter()
                        .map(|(pk, _)| pk.rt())
                        .collect::<Result<Vec<_>, _>>()?;
                    group_client.add_group_members(&signer_ids).await?;
                }

                for (identity, role) in &members {
                    group_store::add_group_member(
                        &datastore,
                        &group_id,
                        identity,
                        role.clone(),
                    )?;
                }

                info!(?group_id, count = members.len(), %requester, "members added to group");

                Ok(())
            }
            .into_actor(self),
        )
    }
}
