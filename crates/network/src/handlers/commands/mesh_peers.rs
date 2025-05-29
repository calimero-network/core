use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::MeshPeers;

use crate::NetworkManager;

impl Handler<MeshPeers> for NetworkManager {
    type Result = <MeshPeers as Message>::Result;

    fn handle(&mut self, MeshPeers(topic): MeshPeers, _ctx: &mut Context<Self>) -> Self::Result {
        self.swarm
            .behaviour()
            .gossipsub
            .mesh_peers(&topic)
            .copied()
            .collect()
    }
}
