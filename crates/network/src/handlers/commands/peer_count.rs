use actix::{Context, Handler};
use calimero_network_primitives::messages::PeerCount;

use crate::NetworkManager;

impl Handler<PeerCount> for NetworkManager {
    type Result = usize;

    fn handle(&mut self, _msg: PeerCount, _ctx: &mut Context<Self>) -> usize {
        self.swarm.connected_peers().count()
    }
}
