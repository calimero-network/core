use actix::{Context, Handler, Message};
use libp2p::gossipsub::TopicHash;

use crate::EventLoop;

#[derive(Message, Clone, Debug)]
#[rtype("usize")]
pub struct MeshPeerCount(TopicHash);

impl Handler<MeshPeerCount> for EventLoop {
    type Result = usize;

    fn handle(&mut self, MeshPeerCount(topic): MeshPeerCount, _ctx: &mut Context<Self>) -> usize {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .mesh_peers(&topic)
            .count()
    }
}
