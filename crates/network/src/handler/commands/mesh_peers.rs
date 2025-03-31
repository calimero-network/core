use actix::{Context, Handler};
use calimero_network_primitives::messages::MeshPeers;
use libp2p::PeerId;

use crate::NetworkManager;

impl Handler<MeshPeers> for NetworkManager {
    type Result = Vec<PeerId>;

    fn handle(&mut self, MeshPeers(topic): MeshPeers, _ctx: &mut Context<Self>) -> Vec<PeerId> {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .mesh_peers(&topic)
            .copied()
            .collect()
    }
}
