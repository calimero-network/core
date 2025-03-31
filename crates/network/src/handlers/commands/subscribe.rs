use actix::{Context, Handler};
use calimero_network_primitives::messages::Subscribe;
use eyre::Result as EyreResult;
use libp2p::gossipsub::IdentTopic;

use crate::NetworkManager;

impl Handler<Subscribe> for NetworkManager {
    type Result = EyreResult<IdentTopic>;

    fn handle(
        &mut self,
        Subscribe(topic): Subscribe,
        _ctx: &mut Context<Self>,
    ) -> EyreResult<IdentTopic> {
        let _ignored = self.swarm.behaviour_mut().gossipsub.subscribe(&topic)?;

        Ok(topic)
    }
}
