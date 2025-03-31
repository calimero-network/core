use actix::dev::MessageResponse;
use actix::{Actor, Context, Handler};
use calimero_network_primitives::client::NetworkMessage;
use calimero_network_primitives::messages::{
    Bootstrap, Dial, ListenOn, MeshPeerCount, MeshPeers, OpenStream, PeerCount, Publish, Subscribe,
    Unsubscribe,
};

use crate::NetworkManager;

pub mod bootstrap;
pub mod dial;
pub mod listen;
pub mod mesh_peer_count;
pub mod mesh_peers;
pub mod open_stream;
pub mod peer_count;
pub mod publish;
pub mod subscribe;
pub mod unsubscribe;

impl Handler<NetworkMessage> for NetworkManager
where
    Self: Actor<Context = Context<Self>>,
{
    type Result = ();

    fn handle(&mut self, msg: NetworkMessage, ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkMessage::Dial { request, outcome } => {
                MessageResponse::<Self, Dial>::handle(self.handle(request, ctx), ctx, Some(outcome))
            }
            NetworkMessage::ListenOn { request, outcome } => {
                MessageResponse::<Self, ListenOn>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::Bootstrap { request, outcome } => {
                MessageResponse::<Self, Bootstrap>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::Subscribe { request, outcome } => {
                MessageResponse::<Self, Subscribe>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::Unsubscribe { request, outcome } => {
                MessageResponse::<Self, Unsubscribe>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::Publish { request, outcome } => {
                MessageResponse::<Self, Publish>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::OpenStream { request, outcome } => {
                MessageResponse::<Self, OpenStream>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::PeerCount { request, outcome } => {
                MessageResponse::<Self, PeerCount>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::MeshPeers { request, outcome } => {
                MessageResponse::<Self, MeshPeers>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
            NetworkMessage::MeshPeerCount { request, outcome } => {
                MessageResponse::<Self, MeshPeerCount>::handle(
                    self.handle(request, ctx),
                    ctx,
                    Some(outcome),
                )
            }
        }
    }
}
