use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::SendSpecializedNodeInvitationResponse;
use eyre::eyre;

use crate::NetworkManager;

impl Handler<SendSpecializedNodeInvitationResponse> for NetworkManager {
    type Result = <SendSpecializedNodeInvitationResponse as Message>::Result;

    fn handle(
        &mut self,
        SendSpecializedNodeInvitationResponse { channel, response }: SendSpecializedNodeInvitationResponse,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        self.swarm
            .behaviour_mut()
            .specialized_node_invite
            .send_response(channel, response)
            .map_err(|_| {
                eyre!("Failed to send specialized node invitation response - channel closed")
            })
    }
}
