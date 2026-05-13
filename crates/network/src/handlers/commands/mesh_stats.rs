use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::MeshStats;

use crate::NetworkManager;

impl Handler<MeshStats> for NetworkManager {
    type Result = <MeshStats as Message>::Result;

    fn handle(&mut self, MeshStats: MeshStats, _ctx: &mut Context<Self>) -> Self::Result {
        let gossipsub = &self.swarm.behaviour().gossipsub;
        gossipsub
            .topics()
            .map(|topic_hash| (topic_hash.clone(), gossipsub.mesh_peers(topic_hash).count()))
            .collect()
    }
}
