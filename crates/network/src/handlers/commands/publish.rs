use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Publish;
use libp2p::gossipsub::PublishError;

use crate::NetworkManager;

impl Handler<Publish> for NetworkManager {
    type Result = <Publish as Message>::Result;

    fn handle(
        &mut self,
        Publish { topic, data }: Publish,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        // Cold-start: when no remote peer is yet known to subscribe to
        // `topic`, libp2p returns `NoPeersSubscribedToTopic` and the
        // message is otherwise lost. Queue it in the outbox and let
        // `drain_publish_outbox` re-publish on the next `Subscribed`
        // event for this topic (within `OUTBOX_TTL`).
        let outbox_copy = data.clone();
        match self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic.clone(), data)
        {
            Ok(id) => Ok(Some(id)),
            Err(PublishError::NoPeersSubscribedToTopic) => {
                self.publish_outbox.enqueue(topic, outbox_copy);
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
}
