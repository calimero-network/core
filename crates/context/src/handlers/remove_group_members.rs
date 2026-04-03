use std::collections::BTreeSet;

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_client::group::RemoveGroupMembersRequest;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use tracing::info;

use crate::group_store;
use crate::ContextManager;

impl Handler<RemoveGroupMembersRequest> for ContextManager {
    type Result = ActorResponse<Self, <RemoveGroupMembersRequest as Message>::Result>;

    fn handle(
        &mut self,
        RemoveGroupMembersRequest {
            group_id,
            members,
            requester,
        }: RemoveGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let preflight = match self.governance_preflight(&group_id, requester, true) {
            Ok(p) => p,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            let admin_count = group_store::count_group_admins(&self.datastore, &group_id)?;
            let mut unique_admins_being_removed: BTreeSet<PublicKey> = BTreeSet::new();
            for id in &members {
                let role = group_store::get_group_member_role(&self.datastore, &group_id, id)?;
                if role == Some(GroupMemberRole::Admin) {
                    unique_admins_being_removed.insert(*id);
                }
            }

            if admin_count <= unique_admins_being_removed.len() {
                bail!("cannot remove all admins from group '{group_id:?}': at least one admin must remain");
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let self_identity = self.node_namespace_identity(&group_id).map(|(pk, _)| pk);
        let datastore = preflight.datastore.clone();
        let node_client = preflight.node_client.clone();
        let context_client = self.context_client.clone();
        let sk = preflight.signer_sk();
        let requester = preflight.requester;
        let members = members.clone();

        ActorResponse::r#async(
            async move {
                for identity in &members {
                    group_store::sign_apply_and_publish_removal(
                        &datastore,
                        &node_client,
                        &group_id,
                        &sk,
                        identity,
                    )
                    .await?;
                }
                info!(
                    ?group_id,
                    count = members.len(),
                    %requester,
                    "members removed from group (local governance signed ops)"
                );

                // Unsubscribe if this node's identity was removed
                if let Some(self_pk) = self_identity {
                    if members.iter().any(|pk| *pk == self_pk) {
                        let _ = node_client.unsubscribe_namespace(group_id.to_bytes()).await;
                    }
                }

                let contexts =
                    group_store::enumerate_group_contexts(&datastore, &group_id, 0, usize::MAX)?;

                for context_id in &contexts {
                    if let Err(err) = context_client.sync_context_config(*context_id, None).await {
                        tracing::warn!(
                            ?group_id,
                            %context_id,
                            ?err,
                            "failed to sync context after group member removal"
                        );
                    }
                }

                Ok(())
            }
            .into_actor(self),
        )
    }
}
