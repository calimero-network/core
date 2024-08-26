use calimero_primitives::identity::PeerId;
use libp2p::core::transport::ListenerId;
pub use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
pub use libp2p::identity::Keypair;
use multiaddr::Multiaddr;

use crate::stream::Stream;

#[derive(Debug)]
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
    Message {
        id: MessageId,
        message: Message,
    },
    StreamOpened {
        peer_id: PeerId,
        stream: Box<Stream>,
    },
}

#[derive(Debug)]
#[non_exhaustive]
pub struct Message {
    pub source: Option<PeerId>,
    pub data: Vec<u8>,
}

impl From<libp2p::gossipsub::Message> for Message {
    fn from(message: libp2p::gossipsub::Message) -> Self {
        Self {
            source: message.source.map(PeerId::from),
            data: message.data,
        }
    }
}
