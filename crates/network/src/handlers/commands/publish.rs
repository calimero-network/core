use actix::{Context, Handler, Message};
use calimero_network_primitives::messages::Publish;
use libp2p::gossipsub::{MessageId, PublishError};

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
        // be able to enqueue the payload for retry on that path.
        //
        // On the queued path we synthesise an empty `MessageId` so
        // callers don't have to special-case `Ok(queued)`. This keeps
        // the public publish contract identical to `master` —
        // governance, ack, and state-delta callers already handle
        // delivery as eventually-consistent at their own layers
        // (`governance_broadcast::publish_and_await_ack_namespace`,
        // `NodeClient::broadcast` + sync-pull) so an indistinguishable
        // `Ok` is correct here.
        let outbox_copy = data.clone();
        match self
            .swarm
            .behaviour_mut()
            .gossipsub
            .publish(topic.clone(), data)
        {
            Ok(id) => Ok(id),
            Err(PublishError::NoPeersSubscribedToTopic) => {
                self.publish_outbox.enqueue(topic, outbox_copy);
                Ok(MessageId::new(&[]))
            }
            Err(e) => Err(e.into()),
        }
    }
}
