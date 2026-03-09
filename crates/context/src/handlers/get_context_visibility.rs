use actix::{ActorResponse, Handler, Message};
use calimero_context_primitives::group::{
    GetContextVisibilityRequest, GetContextVisibilityResponse,
};
use calimero_primitives::identity::PublicKey;
use eyre::bail;

use crate::group_store;
use crate::ContextManager;

impl Handler<GetContextVisibilityRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetContextVisibilityRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetContextVisibilityRequest {
            group_id,
            context_id,
        }: GetContextVisibilityRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<GetContextVisibilityResponse> {
            if group_store::load_group_meta(&self.datastore, &group_id)?.is_none() {
                bail!("group '{group_id:?}' not found");
            }

            let (mode_u8, creator_bytes) =
                group_store::get_context_visibility(&self.datastore, &group_id, &context_id)?
                    .ok_or_else(|| eyre::eyre!("context visibility not found"))?;

            let mode = match mode_u8 {
                0 => calimero_context_config::VisibilityMode::Open,
                1 => calimero_context_config::VisibilityMode::Restricted,
                _ => bail!("invalid visibility mode value: {mode_u8}"),
            };

            Ok(GetContextVisibilityResponse {
                mode,
                creator: PublicKey::from(creator_bytes),
            })
        })();

        ActorResponse::reply(result)
    }
}
