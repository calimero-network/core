use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{GetMemberCapabilitiesRequest, GetMemberCapabilitiesResponse};
use calimero_governance_store::{MembershipRepository, MetaRepository};
use eyre::bail;

use crate::ContextManager;
use calimero_governance_store;

impl Handler<GetMemberCapabilitiesRequest> for ContextManager {
    type Result = ActorResponse<Self, <GetMemberCapabilitiesRequest as Message>::Result>;

    fn handle(
        &mut self,
        GetMemberCapabilitiesRequest { group_id, member }: GetMemberCapabilitiesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<GetMemberCapabilitiesResponse> {
            if MetaRepository::new(&self.datastore)
                .load(&group_id)?
                .is_none()
            {
                // Typed, so the admin API answers 404. As a bare `bail!` this
                // reached the caller as a generic 500, and a control-plane
                // script read that as "already left" and continued past a real
                // failure.
                bail!(crate::error::ContextError::GroupNotFound {
                    group_id: format!("{group_id:?}"),
                });
            }

            let Some(capabilities) = MembershipRepository::new(&self.datastore)
                .effective_capabilities(&group_id, &member)?
            else {
                // Same category: the caller asked about something that is not
                // there. `MemberNotFound` already maps to 404.
                bail!(calimero_governance_store::MembershipError::MemberNotFound {
                    group_id: format!("{group_id:?}"),
                    member: member.to_string(),
                });
            };

            Ok(GetMemberCapabilitiesResponse { capabilities })
        })();

        ActorResponse::reply(result)
    }
}
