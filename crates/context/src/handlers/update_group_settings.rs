use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::UpdateGroupSettingsRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<UpdateGroupSettingsRequest> for ContextManager {
    type Result = ActorResponse<Self, <UpdateGroupSettingsRequest as Message>::Result>;

    fn handle(
        &mut self,
        UpdateGroupSettingsRequest {
            group_id,
            requester,
            upgrade_policy,
        }: UpdateGroupSettingsRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let node_identity = self.node_group_identity();

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

        if let Some((_, node_sk)) = node_identity {
            let _ = group_store::store_group_signing_key(
                &self.datastore,
                &group_id,
                &requester,
                &node_sk,
            );
        }

        if let Err(err) = (|| -> eyre::Result<()> {
            let Some(_meta) = group_store::load_group_meta(&self.datastore, &group_id)? else {
                bail!("group '{group_id:?}' not found");
            };

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;
            group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        let datastore = self.datastore.clone();
        let node_client = self.node_client.clone();
        let effective_signing_key = node_identity
            .map(|(_, sk)| sk)
            .or_else(|| {
                group_store::get_group_signing_key(&self.datastore, &group_id, &requester)
                    .ok()
                    .flatten()
            });

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!(
                        "local group governance requires a signing key for the requester"
                    )
                })?);
                let output = group_store::sign_apply_local_group_op_borsh(
                    &datastore,
                    &group_id,
                    &sk,
                    GroupOp::UpgradePolicySet {
                        policy: upgrade_policy,
                    },
                )?;
                node_client
                    .publish_signed_group_op(group_id.to_bytes(), output.delta_id, output.parent_ids, output.bytes)
                    .await?;

                let _ = node_client
                    .broadcast_group_mutation(
                        group_id.to_bytes(),
                        GroupMutationKind::SettingsUpdated,
                    )
                    .await;
                Ok(())
            }
            .into_actor(self),
        )
    }
}
