use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::MeshPeerCount;

use crate::NetworkManager;

impl Handler<MeshPeerCount> for NetworkManager {
    type Result = <MeshPeerCount as Message>::Result;

    fn handle(
        &mut self,
        MeshPeerCount(topic): MeshPeerCount,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        self.swarm.behaviour().gossipsub.mesh_peers(&topic).count()
    }
}
