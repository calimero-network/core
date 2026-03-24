use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetMemberAliasRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<SetMemberAliasRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberAliasRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberAliasRequest {
            group_id,
            member,
            alias,
            requester,
        }: SetMemberAliasRequest,
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

        let node_sk = node_identity.map(|(_, sk)| sk);
        let signing_key = node_sk;

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            if requester != member {
                bail!("members may only set their own alias");
            }

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

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
        let alias_for_log = alias.clone();

        ActorResponse::r#async(
            async move {
                let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                    eyre::eyre!(
                        "local group governance requires a signing key for the requester"
                    )
                })?);
                let (bytes, delta_id, parent_ids) = group_store::sign_apply_local_group_op_borsh(
                    &datastore,
                    &group_id,
                    &sk,
                    GroupOp::MemberAliasSet {
                        member,
                        alias: alias.clone(),
                    },
                )?;
                node_client
                    .publish_signed_group_op(group_id.to_bytes(), delta_id, parent_ids, bytes)
                    .await?;

                let _ = node_client
                    .broadcast_group_mutation(
                        group_id.to_bytes(),
                        GroupMutationKind::MemberAliasSet {
                            member: *member,
                            alias,
                        },
                    )
                    .await;

                info!(
                    ?group_id,
                    %member,
                    %alias_for_log,
                    "group member alias set"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
