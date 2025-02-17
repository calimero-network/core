use actix::{Context, Handler, Message};
use eyre::{bail, Result as EyreResult};
use libp2p::gossipsub::IdentTopic;

use crate::EventLoop;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<IdentTopic>")]
pub struct Subscribe(IdentTopic);

impl Handler<Subscribe> for EventLoop {
    type Result = EyreResult<IdentTopic>;

    fn handle(
        &mut self,
        Subscribe(topic): Subscribe,
        _ctx: &mut Context<Self>,
    ) -> EyreResult<IdentTopic> {
        match self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
            Ok(_) => Ok(topic),
            Err(e) => bail!(e),
        }
    }
}
