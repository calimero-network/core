use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Subscribe;

use crate::NetworkManager;

impl Handler<Subscribe> for NetworkManager {
    type Result = <Subscribe as Message>::Result;

    fn handle(&mut self, Subscribe(topic): Subscribe, _ctx: &mut Context<Self>) -> Self::Result {
        let newly_subscribed = self.swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

        // A topic subscribed after the last rendezvous registration round
        // is invisible on the server until something unrelated re-registers
        // (TTL expiry, reachability flip, restart) — so peers joining this
        // overlay would discover nobody under its key. Extend our existing
        // registrations with the new key right away.
        if newly_subscribed {
            self.register_new_overlay_topic(topic.hash().as_str());
        }

        Ok(topic)
    }
}
