use actix::Handler;
use calimero_network_primitives::messages::NetworkMessage;
use calimero_utils_actix::forward_handler;

use crate::NetworkManager;

mod bootstrap;
mod dial;
mod listen;
mod mesh_peer_count;
mod mesh_peers;
mod open_stream;
mod peer_count;
mod publish;
mod subscribe;
mod unsubscribe;

impl Handler<NetworkMessage> for NetworkManager {
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkMessage::Dial { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::ListenOn { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::Bootstrap { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::Subscribe { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::Unsubscribe { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::Publish { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::OpenStream { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::PeerCount { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::MeshPeers { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
            NetworkMessage::MeshPeerCount { request, outcome } => {
                forward_handler(self, ctx, request, outcome);
            }
        }
    }
}
