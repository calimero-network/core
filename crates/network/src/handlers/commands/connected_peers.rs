use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::ConnectedPeers;
use libp2p::PeerId;

use crate::NetworkManager;

impl Handler<ConnectedPeers> for NetworkManager {
    type Result = <ConnectedPeers as Message>::Result;

    fn handle(&mut self, _msg: ConnectedPeers, _ctx: &mut Context<Self>) -> Vec<PeerId> {
        self.swarm.connected_peers().copied().collect()
    }
}
