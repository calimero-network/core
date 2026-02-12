//! Specialized node invite request-response protocol event handler

use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::specialized_node_invite::{
    SpecializedNodeInvitationResponse, VerificationRequest,
};
use libp2p::request_response::Event;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, NetworkManager};

impl EventHandler<Event<VerificationRequest, SpecializedNodeInvitationResponse>>
    for NetworkManager
{
    fn handle(&mut self, event: Event<VerificationRequest, SpecializedNodeInvitationResponse>) {
        debug!("{}: {:?}", "specialized_node_invite".yellow(), event);

        match event {
            Event::Message { peer, message, .. } => match message {
                libp2p::request_response::Message::Request {
                    request_id,
                    request,
                    channel,
                } => {
                    debug!(
                        %peer,
                        ?request_id,
                        nonce = %hex::encode(request.nonce()),
                        "Received specialized node verification request"
                    );
                    // Forward to NodeManager for handling
                    let _ignored = self.event_dispatcher.dispatch(
                        NetworkEvent::SpecializedNodeVerificationRequest {
                            peer_id: peer,
                            request_id,
                            request,
                            channel,
                        },
                    );
                }
                libp2p::request_response::Message::Response {
                    request_id,
                    response,
                } => {
                    debug!(
                        %peer,
                        ?request_id,
                        has_invitation = response.invitation_bytes.is_some(),
                        has_error = response.error.is_some(),
                        "Received specialized node invitation response"
                    );
                    // Forward to NodeManager for handling
                    let _ignored = self.event_dispatcher.dispatch(
                        NetworkEvent::SpecializedNodeInvitationResponse {
                            peer_id: peer,
                            request_id,
                            response,
                        },
                    );
                }
            },
            Event::OutboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                debug!(
                    %peer,
                    ?request_id,
                    %error,
                    "Specialized node invite outbound failure"
                );
            }
            Event::InboundFailure {
                peer,
                request_id,
                error,
                ..
            } => {
                debug!(
                    %peer,
                    ?request_id,
                    %error,
                    "Specialized node invite inbound failure"
                );
            }
            Event::ResponseSent {
                peer, request_id, ..
            } => {
                debug!(
                    %peer,
                    ?request_id,
                    "Specialized node invite response sent"
                );
            }
        }
    }
}
