use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
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
        }: JoinGroupRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // 1. Decode the invitation payload
            let (group_id_bytes, inviter_identity, invitee_identity, expiration) =
                invitation_payload.parts().map_err(|err| {
                    eyre::eyre!("failed to decode group invitation payload: {err}")
                })?;

            let group_id = ContextGroupId::from(group_id_bytes);

            // 2. Group must exist
            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // 3. Inviter must still be an admin
            if !group_store::is_group_admin(&self.datastore, &group_id, &inviter_identity)? {
                bail!("inviter is no longer an admin of this group");
            }

            // 4. If targeted invitation, verify joiner matches invitee
            if let Some(expected_invitee) = invitee_identity {
                if expected_invitee != joiner_identity {
                    bail!("this invitation is for a different identity");
                }
            }

            // 5. Check expiration
            if let Some(exp) = expiration {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if now > exp {
                    bail!("invitation has expired");
                }
            }

            // 6. Check if joiner is already a member
            if group_store::check_group_membership(&self.datastore, &group_id, &joiner_identity)? {
                bail!("identity is already a member of this group");
            }

            // 7. Add joiner as Member (not Admin)
            group_store::add_group_member(
                &self.datastore,
                &group_id,
                &joiner_identity,
                GroupMemberRole::Member,
            )?;

            info!(
                ?group_id,
                %joiner_identity,
                %inviter_identity,
                "new member joined group via invitation"
            );

            Ok(JoinGroupResponse {
                group_id,
                member_identity: joiner_identity,
            })
        })();

        ActorResponse::reply(result)
    }
}
