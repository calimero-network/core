use actix::{Context, Handler};
use calimero_network_primitives::messages::Unsubscribe;
use eyre::Result as EyreResult;
use libp2p::gossipsub::IdentTopic;

use crate::NetworkManager;

impl Handler<Unsubscribe> for NetworkManager {
    type Result = EyreResult<IdentTopic>;

    fn handle(
        &mut self,
        Unsubscribe(topic): Unsubscribe,
        _ctx: &mut Context<Self>,
    ) -> EyreResult<IdentTopic> {
        let _ignored = self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic)?;

        Ok(topic)
    }
}
