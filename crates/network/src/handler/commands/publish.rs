use actix::{Context, Handler};
use calimero_network_primitives::messages::Publish;
use eyre::Result as EyreResult;
use libp2p::gossipsub::MessageId;

use crate::NetworkManager;

impl Handler<Publish> for NetworkManager {
    type Result = EyreResult<MessageId>;

    fn handle(
        &mut self,
        Publish { topic, data }: Publish,
        _ctx: &mut Context<Self>,
    ) -> EyreResult<MessageId> {
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic, data)
            .map_err(Into::into)
    }
}
