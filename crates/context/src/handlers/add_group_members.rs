use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::AddGroupMembersRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_primitives::identity::PrivateKey;
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
        }: AddGroupMembersRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

        // Resolve requester: use provided value or fall back to node group identity
        let requester = match requester {
            Some(pk) => pk,
            None => match node_identity {
                Some((pk, _)) => pk,
                None => {
                    return ActorResponse::reply(Err(eyre::eyre!(
                        "requester not provided and node has no configured group identity"
                    )))
                }
            },
        };

        // Resolve signing_key from node identity key
        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        // Sync validation
        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group not found");
            }
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }
            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        // Auto-store signing key for future use
        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let effective_signing_key = signing_key.or_else(|| {
            group_store::get_group_signing_key(&self.datastore, &group_id, &requester)
                .ok()
                .flatten()
        });
        let members = members.clone();

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!("local group governance requires a signing key for the requester")
                })?);
                for (identity, role) in &members {
                    group_store::sign_apply_and_publish(
                        &datastore,
                        &node_client,
                        &group_id,
                        &sk,
                        GroupOp::MemberAdded {
                            member: *identity,
                            role: role.clone(),
                        },
                    )
                    .await?;
                }
                info!(
                    ?group_id,
                    count = members.len(),
                    %requester,
                    "members added to group (local governance signed ops)"
                );
                Ok(())
            }
            .into_actor(self),
        )
    }
}
