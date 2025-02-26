use actix::{Context, Handler, Message};
use libp2p::gossipsub::TopicHash;

use crate::NetworkManager;

#[derive(Message, Clone, Debug)]
#[rtype(usize)]
pub struct MeshPeerCount(TopicHash);

impl From<TopicHash> for MeshPeerCount {
    fn from(topic: TopicHash) -> Self {
        Self(topic)
    }
}

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
