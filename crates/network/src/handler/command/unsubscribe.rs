use actix::{Context, Handler, Message};
use eyre::{bail, Result as EyreResult};
use libp2p::gossipsub::IdentTopic;

use crate::EventLoop;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<IdentTopic>")]
pub struct Unsubscribe(IdentTopic);

impl Handler<Unsubscribe> for EventLoop {
    type Result = EyreResult<IdentTopic>;

    fn handle(
        &mut self,
        Unsubscribe(topic): Unsubscribe,
        _ctx: &mut Context<Self>,
    ) -> EyreResult<IdentTopic> {
        match self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic) {
            Ok(_) => Ok(topic),
            Err(e) => bail!(e),
        }
    }
}
