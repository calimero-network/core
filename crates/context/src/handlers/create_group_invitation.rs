use std::time::{SystemTime, UNIX_EPOCH};

use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{
    CreateGroupInvitationRequest, CreateGroupInvitationResponse,
};
use calimero_primitives::context::GroupInvitationPayload;

use crate::{group_store, ContextManager};

impl Handler<CreateGroupInvitationRequest> for ContextManager {
    type Result = ActorResponse<Self, <CreateGroupInvitationRequest as Message>::Result>;

    fn handle(
        &mut self,
        CreateGroupInvitationRequest {
            group_id,
            requester,
            invitee_identity,
            expiration,
        }: CreateGroupInvitationRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| {
            // 1. Group must exist
            let _meta = group_store::load_group_meta(&self.datastore, &group_id)?
                .ok_or_else(|| eyre::eyre!("group not found"))?;

            // 2. Requester must be admin
            group_store::require_group_admin(&self.datastore, &group_id, &requester)?;

            // 3. Verify node holds the requester's signing key
            group_store::require_group_signing_key(&self.datastore, &group_id, &requester)?;

            // 4. Validate expiration (if set, must be in the future)
            if let Some(exp) = expiration {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if exp <= now {
                    eyre::bail!("expiration must be a future unix timestamp");
                }
            }

            // 5. Extract contract coordinates
            let params = self
                .external_config
                .params
                .get("near")
                .ok_or_else(|| eyre::eyre!("no 'near' protocol config"))?;

            // 6. Build the invitation payload
            let payload = GroupInvitationPayload::new(
                group_id.to_bytes(),
                requester,
                invitee_identity,
                expiration,
                "near",
                &params.network,
                &params.contract_id,
            )?;

            Ok(CreateGroupInvitationResponse { payload })
        })();

        ActorResponse::reply(result)
    }
}
