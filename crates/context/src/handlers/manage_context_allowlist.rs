use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_primitives::group::ManageContextAllowlistRequest;
use calimero_context_primitives::local_governance::GroupOp;
use calimero_node_primitives::sync::GroupMutationKind;
use calimero_primitives::identity::PrivateKey;
use eyre::bail;
use tracing::info;

use crate::config::GroupGovernanceMode;
use crate::group_store;
use crate::ContextManager;

impl Handler<ManageContextAllowlistRequest> for ContextManager {
    type Result = ActorResponse<Self, <ManageContextAllowlistRequest as Message>::Result>;

    fn handle(
        &mut self,
        ManageContextAllowlistRequest {
            group_id,
            context_id,
            add,
            remove,
            requester,
        }: ManageContextAllowlistRequest,
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

            let is_admin = group_store::is_group_admin(&self.datastore, &group_id, &requester)?;
            if !is_admin {
                if let Some((_, creator_bytes)) =
                    group_store::get_context_visibility(&self.datastore, &group_id, &context_id)?
                {
                    if creator_bytes != *requester {
                        bail!("only admin or context creator can manage allowlist");
                    }
                } else {
                    bail!("context visibility not found for context in group");
                }
            }

            if signing_key.is_none() {
                group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;
            }

            if group_governance != GroupGovernanceMode::Local {
                for member in &add {
                    group_store::add_to_context_allowlist(
                        &self.datastore,
                        &group_id,
                        &context_id,
                        member,
                    )?;
                }

                for member in &remove {
                    group_store::remove_from_context_allowlist(
                        &self.datastore,
                        &group_id,
                        &context_id,
                        member,
                    )?;
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

        let merged_allowlist = if group_governance == GroupGovernanceMode::Local {
            let mut m = match group_store::list_context_allowlist(
                &self.datastore,
                &group_id,
                &context_id,
            ) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            };
            for r in &remove {
                m.retain(|x| x != r);
            }
            for a in &add {
                if !m.contains(a) {
                    m.push(*a);
                }
            }
            m
        } else {
            match group_store::list_context_allowlist(&self.datastore, &group_id, &context_id) {
                Ok(v) => v,
                Err(err) => return ActorResponse::reply(Err(err)),
            }
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
                        let members = merged_allowlist.clone();
                        let bytes = group_store::sign_apply_local_group_op_borsh(
                            &datastore,
                            &group_id,
                            &sk,
                            GroupOp::ContextAllowlistReplaced {
                                context_id,
                                members,
                            },
                        )?;
                        node_client
                            .publish_signed_group_op(group_id.to_bytes(), bytes)
                            .await?;
                    }
                    GroupGovernanceMode::External => {
                        if let Some(client_result) = group_client_result {
                            let mut group_client = client_result?;
                            let add_signer_ids: Vec<calimero_context_config::types::SignerId> = add
                                .iter()
                                .map(|pk| pk.rt())
                                .collect::<Result<Vec<_>, _>>()?;
                            let remove_signer_ids: Vec<calimero_context_config::types::SignerId> =
                                remove
                                    .iter()
                                    .map(|pk| pk.rt())
                                    .collect::<Result<Vec<_>, _>>()?;
                            group_client
                                .manage_context_allowlist(
                                    context_id,
                                    add_signer_ids,
                                    remove_signer_ids,
                                )
                                .await?;
                        }

                        let members_raw: Vec<[u8; 32]> =
                            merged_allowlist.iter().map(|pk| **pk).collect();
                        let _ = node_client
                            .broadcast_group_mutation(
                                group_id.to_bytes(),
                                GroupMutationKind::ContextAllowlistSet {
                                    context_id: *context_id,
                                    members: members_raw,
                                },
                            )
                            .await;
                    }
                }

                info!(
                    ?group_id,
                    %context_id,
                    added = add.len(),
                    removed = remove.len(),
                    "context allowlist updated"
                );

                Ok(())
            }
            .into_actor(self),
        )
    }
}
