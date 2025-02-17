use actix::{Context, Handler, Message};

use crate::EventLoop;

#[derive(Message, Clone, Copy, Debug)]
#[rtype("usize")]
pub struct PeerCount;

impl Handler<PeerCount> for EventLoop {
    type Result = usize;

    fn handle(&mut self, _msg: PeerCount, _ctx: &mut Context<Self>) -> usize {
        self.swarm.connected_peers().count()
    }
}
