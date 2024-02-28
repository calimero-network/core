pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
pub use libp2p::identity::PeerId;

#[derive(Debug)]
pub enum NetworkEvent {
    Subscribed { peer_id: PeerId, topic: TopicHash },
    Message { id: MessageId, message: Message },
}
