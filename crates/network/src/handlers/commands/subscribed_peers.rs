use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::SubscribedPeers;

use crate::NetworkManager;

impl Handler<SubscribedPeers> for NetworkManager {
    type Result = <SubscribedPeers as Message>::Result;

    fn handle(
        &mut self,
        SubscribedPeers(topic): SubscribedPeers,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        // The full subscriber set, not the grafted mesh: `all_peers`
        // yields every connected peer with the topics it subscribed to
        // (populated on the `SUBSCRIBE` control message, no GRAFT needed).
        // Filter to the ones following `topic`. This is what sync uses to
        // find a reconcile partner, so a peer that's connected + subscribed
        // is always reachable even if it isn't (yet/still) in our mesh.
        self.swarm
            .behaviour()
            .gossipsub
            .all_peers()
            .filter_map(|(peer_id, topics)| topics.contains(&&topic).then_some(*peer_id))
            .collect()
    }
}
