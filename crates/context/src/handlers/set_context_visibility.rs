use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_primitives::group::SetContextVisibilityRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::bail;
use tracing::info;

use crate::config::GroupGovernanceMode;
use crate::group_store;
use crate::ContextManager;

impl Handler<SetContextVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <SetContextVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        SetContextVisibilityRequest {
            group_id,
            context_id,
            mode,
            requester,
        }: SetContextVisibilityRequest,
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

        let mode_u8 = match mode {
            calimero_context_config::VisibilityMode::Open => 0u8,
            calimero_context_config::VisibilityMode::Restricted => 1u8,
        };

        if let Err(err) = (|| -> eyre::Result<()> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            let is_admin = group_store::is_group_admin(&self.datastore, &group_id, &requester)?;
            if !is_admin {
                if let Some((_, creator_bytes)) =
                    group_store::get_context_visibility(&self.datastore, &group_id, &context_id)?
                {
                    if creator_bytes != *requester {
                        bail!("only admin or context creator can set visibility");
                    }
                } else {
                    bail!("context visibility not found for context in group");
                }
            }

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            if group_governance != GroupGovernanceMode::Local {
                let creator = group_store::get_context_visibility(&self.datastore, &group_id, &context_id)?
                    .map(|(_, c)| c)
                    .unwrap_or(*requester);

                group_store::set_context_visibility(
                    &self.datastore,
                    &group_id,
                    &context_id,
                    mode_u8,
                    creator,
                )?;

                if mode == calimero_context_config::VisibilityMode::Restricted {
                    let creator_pk = PublicKey::from(creator);
                    if !group_store::check_context_allowlist(
                        &self.datastore,
                        &group_id,
                        &context_id,
                        &creator_pk,
                    )? {
                        group_store::add_to_context_allowlist(
                            &self.datastore,
                            &group_id,
                            &context_id,
                            &creator_pk,
                        )?;
                    }
                }
            }

            Ok(())
        })() {
            return ActorResponse::reply(Err(err));
        }

        if let Some(ref sk) = signing_key {
            let _ =
                group_store::store_group_signing_key(&self.datastore, &group_id, &requester, sk);
        }

        let creator_pk = {
            let creator_bytes = group_store::get_context_visibility(&self.datastore, &group_id, &context_id)
                .ok()
                .flatten()
                .map(|(_, c)| c)
                .unwrap_or(*requester);
            PublicKey::from(creator_bytes)
        };

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
                            GroupOp::ContextVisibilitySet {
                                context_id,
                                mode: mode_u8,
                                creator: creator_pk,
                            },
                        )?;
                        node_client
                            .publish_signed_group_op(group_id.to_bytes(), bytes)
                            .await?;
                    }
                    GroupGovernanceMode::External => {
                        if let Some(client_result) = group_client_result {
                            let mut group_client = client_result?;
                            group_client
                                .set_context_visibility(context_id, mode)
                                .await?;
                        }

                        let _ = node_client
                            .broadcast_group_mutation(
                                group_id.to_bytes(),
                                GroupMutationKind::ContextVisibilitySet {
                                    context_id: *context_id,
                                    mode: mode_u8,
                                    creator: *creator_pk,
                                },
                            )
                            .await;
                    }
                }

                info!(?group_id, %context_id, ?mode, "context visibility updated");

                Ok(())
            }
            .into_actor(self),
        )
    }
}
