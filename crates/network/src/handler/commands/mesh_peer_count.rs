use actix::{Context, Handler};
use calimero_network_primitives::messages::MeshPeerCount;

use crate::NetworkManager;

impl Handler<MeshPeerCount> for NetworkManager {
    type Result = usize;

    fn handle(&mut self, MeshPeerCount(topic): MeshPeerCount, _ctx: &mut Context<Self>) -> usize {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .mesh_peers(&topic)
            .count()
    }
}
