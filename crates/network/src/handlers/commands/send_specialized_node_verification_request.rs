use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::SendSpecializedNodeVerificationRequest;

use crate::NetworkManager;

impl Handler<SendSpecializedNodeVerificationRequest> for NetworkManager {
    type Result = <SendSpecializedNodeVerificationRequest as Message>::Result;

    fn handle(
        &mut self,
        SendSpecializedNodeVerificationRequest { peer_id, request }: SendSpecializedNodeVerificationRequest,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        let request_id = self
            .swarm
            .behaviour_mut()
            .specialized_node_invite
            .send_request(&peer_id, request);

        Ok(request_id)
    }
}
