use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::PeerCount;

use crate::NetworkManager;

impl Handler<PeerCount> for NetworkManager {
    type Result = <PeerCount as Message>::Result;

    fn handle(&mut self, _msg: PeerCount, _ctx: &mut Context<Self>) -> usize {
        self.swarm.connected_peers().count()
    }
}
