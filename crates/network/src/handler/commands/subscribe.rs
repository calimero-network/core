use actix::{Context, Handler, Message};
use eyre::Result as EyreResult;
use libp2p::gossipsub::IdentTopic;

use crate::NetworkManager;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<IdentTopic>")]
pub struct Subscribe(IdentTopic);

impl From<IdentTopic> for Subscribe {
    fn from(topic: IdentTopic) -> Self {
        Self(topic)
    }
}
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
