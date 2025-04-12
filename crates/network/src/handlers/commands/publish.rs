use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Publish;

use crate::NetworkManager;

impl Handler<Publish> for NetworkManager {
    type Result = <Publish as Message>::Result;

    fn handle(
        &mut self,
        Publish { topic, data }: Publish,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, data)
            .map_err(Into::into)
    }
}
