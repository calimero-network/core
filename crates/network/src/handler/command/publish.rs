use actix::{Context, Handler, Message};
use eyre::{bail, Result as EyreResult};
use libp2p::gossipsub::{MessageId, TopicHash};

use crate::NetworkManager;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<MessageId>")]
pub struct Publish {
    topic: TopicHash,
    data: Vec<u8>,
}

impl From<(TopicHash, Vec<u8>)> for Publish {
    fn from((topic, data): (TopicHash, Vec<u8>)) -> Self {
        Self { topic, data }
    }
}

impl Handler<Publish> for NetworkManager {
    type Result = EyreResult<MessageId>;

    fn handle(
        &mut self,
        Publish { topic, data }: Publish,
        _ctx: &mut Context<Self>,
    ) -> EyreResult<MessageId> {
        match self.swarm.behaviour_mut().gossipsub.publish(topic, data) {
            Ok(id) => Ok(id),
            Err(err) => bail!(err),
        }
    }
}
