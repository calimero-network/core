use libp2p::core::transport;
pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
pub use libp2p::identity::PeerId;

use super::stream;

#[derive(Debug)]
pub enum NetworkEvent {
    ListeningOn {
        listener_id: transport::ListenerId,
        address: libp2p::Multiaddr,
    },
    Subscribed {
        peer_id: PeerId,
        topic: TopicHash,
    },
    Message {
        id: MessageId,
        message: Message,
    },
    StreamOpened {
        peer_id: PeerId,
        stream: Box<stream::Stream>,
    },
}
