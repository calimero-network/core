use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::group::SetMemberCapabilitiesRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::info;

use crate::config::GroupGovernanceMode;
use crate::group_store;
use crate::ContextManager;

impl Handler<SetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetMemberCapabilitiesRequest {
            group_id,
            member,
            capabilities,
            requester,
        }: SetMemberCapabilitiesRequest,
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
        let group_governance = self.group_governance;

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            if group_store::get_group_member_role(&self.datastore, &group_id, &member)?.is_none() {
                bail!("identity is not a member of group '{group_id:?}'");
            }

            if group_governance != GroupGovernanceMode::Local {
                group_store::set_member_capability(&self.datastore, &group_id, &member, capabilities)?;
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
        let group_client_result = match group_governance {
            GroupGovernanceMode::External => {
                effective_signing_key.map(|sk| self.group_client(group_id, sk))
            }
            GroupGovernanceMode::Local => None,
        };

        ActorResponse::r#async(
            async move {
                match group_governance {
                    GroupGovernanceMode::Local => {
                        let sk = PrivateKey::from(effective_signing_key.ok_or_else(|| {
                            eyre::eyre!(
                                "local group governance requires a signing key for the requester"
                            )
                        })?);
                        let bytes = group_store::sign_apply_local_group_op_borsh(
                            &datastore,
                            &group_id,
                            &sk,
                            GroupOp::MemberCapabilitySet {
                                member,
                                capabilities,
                            },
                        )?;
                        node_client
                            .publish_signed_group_op(group_id.to_bytes(), bytes)
                            .await?;
                    }
                    GroupGovernanceMode::External => {
                        if let Some(client_result) = group_client_result {
                            let mut group_client = client_result?;
                            let signer_id: calimero_context_config::types::SignerId = member.rt()?;
                            group_client
                                .set_member_capabilities(signer_id, capabilities)
                                .await?;
                        }

                        let _ = node_client
                            .broadcast_group_mutation(
                                group_id.to_bytes(),
                                GroupMutationKind::MemberCapabilitySet {
                                    member: *member,
                                    capabilities,
                                },
                            )
                            .await;
                    }
                }

                info!(?group_id, %member, capabilities, "member capabilities updated");

                Ok(())
            }
            .into_actor(self),
        )
    }
}
