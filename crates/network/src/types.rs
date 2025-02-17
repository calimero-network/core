use actix::Message as ActixMessage;
use libp2p::core::transport::ListenerId;
pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
pub use libp2p::identity::PeerId;
use multiaddr::Multiaddr;

use crate::stream::Stream;

#[derive(ActixMessage, Debug)]
#[rtype(result = "()")]
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
