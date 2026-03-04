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
        // Sync validation
        let group_id = match (|| -> eyre::Result<ContextGroupId> {
            let (group_id_bytes, inviter_identity, invitee_identity, expiration) =
                invitation_payload.parts().map_err(|err| {
                    eyre::eyre!("failed to decode group invitation payload: {err}")
                })?;

            let group_id = ContextGroupId::from(group_id_bytes);

            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            if !group_store::is_group_admin(&self.datastore, &group_id, &inviter_identity)? {
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

            if group_store::check_group_membership(
                &self.datastore,
                &group_id,
                &joiner_identity,
            )? {
                bail!("identity is already a member of this group");
            }

            Ok(group_id)
        })() {
            Ok(id) => id,
            Err(err) => return ActorResponse::reply(Err(err)),
        };

        let datastore = self.datastore.clone();
        let group_client_result = signing_key.map(|sk| self.group_client(group_id, sk));

        ActorResponse::r#async(
            async move {
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
