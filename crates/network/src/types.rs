use actix::Message as ActixMessage;
use calimero_network_primitives::stream::Stream;
use libp2p::core::transport::ListenerId;
pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
pub use libp2p::identity::PeerId;
use multiaddr::Multiaddr;

#[derive(ActixMessage, Debug)]
#[rtype("()")]
#[non_exhaustive]
pub enum NetworkEvent {
    ListeningOn {
        listener_id: ListenerId,
        address: Multiaddr,
    },
    Subscribed {
        peer_id: PeerId,
        topic: TopicHash,
    },
    Unsubscribed {
        peer_id: PeerId,
        topic: TopicHash,
    },
    Message {
        id: MessageId,
        message: Message,
    },
    StreamOpened {
        peer_id: PeerId,
        stream: Box<Stream>,
    },
}
