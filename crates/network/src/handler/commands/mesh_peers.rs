use actix::{Context, Handler, Message};
use libp2p::gossipsub::TopicHash;
use libp2p::PeerId;

use crate::NetworkManager;

#[derive(Message, Clone, Debug)]
#[rtype("Vec<PeerId>")]
pub struct MeshPeers(TopicHash);

impl From<TopicHash> for MeshPeers {
    fn from(topic: TopicHash) -> Self {
        Self(topic)
    }
}
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
