use libp2p::core::transport::ListenerId;
pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
use libp2p::Multiaddr;
pub use libp2p::PeerId;
use tokio::sync::oneshot;

use crate::stream::Stream;

#[derive(Debug, actix::Message)]
#[rtype("()")]
pub enum NetworkMessage {
    Dial {
        request: Dial,
        outcome: oneshot::Sender<<Dial as actix::Message>::Result>,
    },
    ListenOn {
        request: ListenOn,
        outcome: oneshot::Sender<<ListenOn as actix::Message>::Result>,
    },
    Bootstrap {
        request: Bootstrap,
        outcome: oneshot::Sender<<Bootstrap as actix::Message>::Result>,
    },
    Subscribe {
        request: Subscribe,
        outcome: oneshot::Sender<<Subscribe as actix::Message>::Result>,
    },
    Unsubscribe {
        request: Unsubscribe,
        outcome: oneshot::Sender<<Unsubscribe as actix::Message>::Result>,
    },
    Publish {
        request: Publish,
        outcome: oneshot::Sender<<Publish as actix::Message>::Result>,
    },
    OpenStream {
        request: OpenStream,
        outcome: oneshot::Sender<<OpenStream as actix::Message>::Result>,
    },
    PeerCount {
        request: PeerCount,
        outcome: oneshot::Sender<<PeerCount as actix::Message>::Result>,
    },
    MeshPeers {
        request: MeshPeers,
        outcome: oneshot::Sender<<MeshPeers as actix::Message>::Result>,
    },
    MeshPeerCount {
        request: MeshPeerCount,
        outcome: oneshot::Sender<<MeshPeerCount as actix::Message>::Result>,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Bootstrap;

impl actix::Message for Bootstrap {
    type Result = eyre::Result<()>;
}

#[derive(Clone, Debug)]
pub struct Dial(pub Multiaddr);

impl actix::Message for Dial {
    type Result = eyre::Result<()>;
}

#[derive(Clone, Debug)]
pub struct ListenOn(pub Multiaddr);

impl actix::Message for ListenOn {
    type Result = eyre::Result<()>;
}

#[derive(Clone, Debug)]
pub struct MeshPeerCount(pub TopicHash);

impl actix::Message for MeshPeerCount {
    type Result = usize;
}

#[derive(Clone, Debug)]
pub struct MeshPeers(pub TopicHash);

impl actix::Message for MeshPeers {
    type Result = Vec<PeerId>;
}

#[derive(Clone, Copy, Debug)]
pub struct OpenStream(pub PeerId);

impl actix::Message for OpenStream {
    type Result = eyre::Result<Stream>;
}

#[derive(Clone, Copy, Debug)]
pub struct PeerCount;

impl actix::Message for PeerCount {
    type Result = usize;
}

#[derive(Clone, Debug)]
pub struct Publish {
    pub topic: TopicHash,
    pub data: Vec<u8>,
}

impl actix::Message for Publish {
    type Result = eyre::Result<MessageId>;
}

#[derive(Clone, Debug)]
pub struct Subscribe(pub IdentTopic);

impl actix::Message for Subscribe {
    type Result = eyre::Result<IdentTopic>;
}

#[derive(Clone, Debug)]
pub struct Unsubscribe(pub IdentTopic);

impl actix::Message for Unsubscribe {
    type Result = eyre::Result<IdentTopic>;
}

#[derive(Debug)]
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

impl actix::Message for NetworkEvent {
    type Result = ();
}
