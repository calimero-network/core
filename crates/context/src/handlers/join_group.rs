use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message, WrapFuture};
use calimero_context_config::repr::ReprTransmute;
use calimero_context_config::types::ContextGroupId;
use calimero_context_primitives::group::{JoinGroupRequest, JoinGroupResponse};
use calimero_primitives::context::GroupMemberRole;
use eyre::bail;
use tracing::info;

use crate::{group_store, ContextManager};

impl Handler<JoinGroupRequest> for ContextManager {
    type Result = ActorResponse<Self, <JoinGroupRequest as Message>::Result>;

    fn handle(
        &mut self,
        JoinGroupRequest {
            invitation_payload,
            joiner_identity,
            signing_key,
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        // Decode invitation
        let (
            group_id_bytes,
            inviter_identity,
            invitee_identity,
            expiration,
            protocol,
            network_id,
            contract_id,
        ) = match invitation_payload.parts() {
            Ok(parts) => parts,
            Err(err) => {
                return ActorResponse::reply(Err(eyre::eyre!(
                    "failed to decode group invitation payload: {err}"
                )));
            }
        };

        let group_id = ContextGroupId::from(group_id_bytes);

        // Check if we need to bootstrap from chain
        let needs_chain_sync = group_store::load_group_meta(&self.datastore, &group_id)
            .map(|opt| opt.is_none())
            .unwrap_or(true);

        // Auto-store signing key if provided
        if let Some(ref sk) = signing_key {
            let _ = group_store::store_group_signing_key(
                &self.datastore,
                &group_id,
                &joiner_identity,
                sk,
            );
        }

        // Resolve effective signing key (provided or previously stored)
        let effective_signing_key = match signing_key {
            Some(sk) => Some(sk),
            None => {
                group_store::get_group_signing_key(&self.datastore, &group_id, &joiner_identity)
                    .ok()
                    .flatten()
            }
        };

        let group_client_result = effective_signing_key.map(|sk| self.group_client(group_id, sk));

        let datastore = self.datastore.clone();
        let context_client = self.context_client.clone();

        ActorResponse::r#async(
            async move {
                // Phase 1: Bootstrap from chain if local state is missing
                if needs_chain_sync {
                    let mut meta = group_store::sync_group_state_from_contract(
                        &datastore,
                        &context_client,
                        &group_id,
                        &protocol,
                        &network_id,
                        &contract_id,
                    )
                    .await?;

                    // Set admin_identity to inviter (who created the invitation)
                    meta.admin_identity = inviter_identity;
                    group_store::save_group_meta(&datastore, &group_id, &meta)?;

                    // Add inviter as admin locally so validation passes
                    group_store::add_group_member(
                        &datastore,
                        &group_id,
                        &inviter_identity,
                        GroupMemberRole::Admin,
                    )?;
                }

                // Phase 2: Validate
                if !group_store::is_group_admin(&datastore, &group_id, &inviter_identity)? {
                    bail!("inviter is no longer an admin of this group");
                }

                if let Some(expected_invitee) = invitee_identity {
                    if expected_invitee != joiner_identity {
                        bail!("this invitation is for a different identity");
                    }
                }

                if let Some(exp) = expiration {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    if now > exp {
                        bail!("invitation has expired");
                    }
                }

                if group_store::check_group_membership(&datastore, &group_id, &joiner_identity)? {
                    bail!("identity is already a member of this group");
                }

                // Phase 3: Contract + local store
                if let Some(client_result) = group_client_result {
                    let mut group_client = client_result?;
                    let signer_id: calimero_context_config::types::SignerId =
                        joiner_identity.rt()?;
                    group_client.add_group_members(&[signer_id]).await?;
                }

                group_store::add_group_member(
                    &datastore,
                    &group_id,
                    &joiner_identity,
                    GroupMemberRole::Member,
                )?;

                info!(
                    ?group_id,
                    %joiner_identity,
                    "new member joined group via invitation"
                );

                Ok(JoinGroupResponse {
                    group_id,
                    member_identity: joiner_identity,
                })
            }
            .into_actor(self),
        )
    }
}
