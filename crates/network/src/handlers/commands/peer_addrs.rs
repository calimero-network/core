use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::PeerAddrs;

use crate::NetworkManager;

impl Handler<PeerAddrs> for NetworkManager {
    type Result = <PeerAddrs as Message>::Result;

    /// Served from the persistent address cache rather than the live swarm.
    ///
    /// A caller asks this to learn where a peer it is NOT connected to can be
    /// reached, so answering from current connections would answer only the
    /// cases that did not need asking. The cache keeps relay-circuit addresses
    /// alongside direct ones, which is what makes a NAT'd peer answerable at
    /// all.
    fn handle(&mut self, PeerAddrs(peer_id): PeerAddrs, _ctx: &mut Context<Self>) -> Self::Result {
        self.peer_cache_addrs_for(&peer_id)
    }
}
