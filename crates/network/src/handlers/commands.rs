use actix::Handler;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_utils_actix::adapters::ActorExt;

use crate::NetworkManager;

mod announce_blob;
mod bootstrap;
mod dial;
mod listen;
mod mesh_peer_count;
mod mesh_peers;
mod open_stream;
mod peer_count;
mod publish;
mod query_blob;
mod request_blob;
mod send_specialized_node_invitation_response;
mod send_specialized_node_verification_request;
mod subscribe;
mod unsubscribe;

impl Handler<NetworkMessage> for NetworkManager {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkMessage::Dial { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::ListenOn { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::Bootstrap { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::Subscribe { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::Unsubscribe { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::Publish { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::OpenStream { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::PeerCount { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::MeshPeers { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::MeshPeerCount { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::AnnounceBlob { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::QueryBlob { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::RequestBlob { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::SendSpecializedNodeVerificationRequest { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
            NetworkMessage::SendSpecializedNodeInvitationResponse { request, outcome } => {
                self.forward_handler(ctx, request, outcome);
            }
        }
    }
}
