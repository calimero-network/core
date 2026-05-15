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
        //
        // `gossipsub.publish` consumes `data` and can still return
        // `NoPeersSubscribedToTopic` *after* the consume (the
        // subscriber set is checked inside `publish`). We pre-clone to
        // be able to enqueue the payload for retry on that path. A
        // mesh-emptiness pre-check could skip the clone in the
        // common case but is racy with gossipsub's internal state —
        // accepting the one clone keeps the contract correct.
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
