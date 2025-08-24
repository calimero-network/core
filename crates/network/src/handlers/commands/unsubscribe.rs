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
        let _ignored = self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic);

        Ok(topic)
    }
}
