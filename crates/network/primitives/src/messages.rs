use actix::Message;
use eyre::Result as EyreResult;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::{Multiaddr, PeerId};
use tokio::sync::oneshot;

use crate::stream::Stream;

#[derive(Debug, Message)]
#[rtype("()")]
pub enum NetworkMessage {
    Dial {
        request: Dial,
        outcome: oneshot::Sender<<Dial as Message>::Result>,
    },
    ListenOn {
        request: ListenOn,
        outcome: oneshot::Sender<<ListenOn as Message>::Result>,
    },
    Bootstrap {
        request: Bootstrap,
        outcome: oneshot::Sender<<Bootstrap as Message>::Result>,
    },
    Subscribe {
        request: Subscribe,
        outcome: oneshot::Sender<<Subscribe as Message>::Result>,
    },
    Unsubscribe {
        request: Unsubscribe,
        outcome: oneshot::Sender<<Unsubscribe as Message>::Result>,
    },
    Publish {
        request: Publish,
        outcome: oneshot::Sender<<Publish as Message>::Result>,
    },
    OpenStream {
        request: OpenStream,
        outcome: oneshot::Sender<<OpenStream as Message>::Result>,
    },
    PeerCount {
        request: PeerCount,
        outcome: oneshot::Sender<<PeerCount as Message>::Result>,
    },
    MeshPeers {
        request: MeshPeers,
        outcome: oneshot::Sender<<MeshPeers as Message>::Result>,
    },
    MeshPeerCount {
        request: MeshPeerCount,
        outcome: oneshot::Sender<<MeshPeerCount as Message>::Result>,
    },
}

#[derive(Clone, Copy, Debug)]
pub struct Bootstrap;

impl Message for Bootstrap {
    type Result = EyreResult<()>;
}

#[derive(Clone, Debug)]
pub struct Dial(pub Multiaddr);

impl Message for Dial {
    type Result = EyreResult<()>;
}

#[derive(Clone, Debug)]
pub struct ListenOn(pub Multiaddr);

impl Message for ListenOn {
    type Result = EyreResult<()>;
}

#[derive(Clone, Debug)]
pub struct MeshPeerCount(pub TopicHash);

impl Message for MeshPeerCount {
    type Result = usize;
}

#[derive(Clone, Debug)]
pub struct MeshPeers(pub TopicHash);

impl Message for MeshPeers {
    type Result = Vec<PeerId>;
}

#[derive(Clone, Copy, Debug)]
pub struct OpenStream(pub PeerId);

impl Message for OpenStream {
    type Result = EyreResult<Stream>;
}

#[derive(Clone, Copy, Debug)]
pub struct PeerCount;

impl Message for PeerCount {
    type Result = usize;
}

#[derive(Clone, Debug)]
pub struct Publish {
    pub topic: TopicHash,
    pub data: Vec<u8>,
}

impl Message for Publish {
    type Result = EyreResult<MessageId>;
}

#[derive(Clone, Debug)]
pub struct Subscribe(pub IdentTopic);

impl Message for Subscribe {
    type Result = EyreResult<IdentTopic>;
}

#[derive(Clone, Debug)]
pub struct Unsubscribe(pub IdentTopic);

impl Message for Unsubscribe {
    type Result = EyreResult<IdentTopic>;
}
