use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Subscribe;

use crate::NetworkManager;

impl Handler<Subscribe> for NetworkManager {
    type Result = <Subscribe as Message>::Result;

    fn handle(&mut self, Subscribe(topic): Subscribe, _ctx: &mut Context<Self>) -> Self::Result {
        let _ignored = self.swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

        Ok(topic)
    }
}
