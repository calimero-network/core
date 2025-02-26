use actix::{Context, Handler, Message};

use crate::NetworkManager;

#[derive(Message, Clone, Copy, Debug)]
#[rtype(usize)]
pub struct PeerCount;

impl Handler<PeerCount> for NetworkManager {
    type Result = usize;

    fn handle(&mut self, _msg: PeerCount, _ctx: &mut Context<Self>) -> usize {
        self.swarm.connected_peers().count()
    }
}
