use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Unsubscribe;

use crate::NetworkManager;

impl Handler<Unsubscribe> for NetworkManager {
    type Result = <Unsubscribe as Message>::Result;

    fn handle(
        &mut self,
        Unsubscribe(topic): Unsubscribe,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        let was_subscribed = self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic);

        // Mirror of the subscribe path: drop our per-overlay rendezvous key
        // so joiners stop being steered toward a node that no longer
        // follows this overlay (the record would otherwise linger until
        // its TTL).
        if was_subscribed {
            self.unregister_dropped_overlay_topic(topic.hash().as_str());
        }

        Ok(topic)
    }
}
