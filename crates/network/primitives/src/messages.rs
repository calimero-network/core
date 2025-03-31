use actix::Message;
use eyre::Result as EyreResult;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::{Multiaddr, PeerId};

use crate::stream::Stream;

#[derive(Message, Clone, Copy, Debug)]
#[rtype("EyreResult<()>")]
pub struct Bootstrap;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<()>")]
pub struct Dial(pub Multiaddr);

impl From<Multiaddr> for Dial {
    fn from(addr: Multiaddr) -> Self {
        Self(addr)
    }
}

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<()>")]
pub struct ListenOn(pub Multiaddr);

impl From<Multiaddr> for ListenOn {
    fn from(addr: Multiaddr) -> Self {
        Self(addr)
    }
}

#[derive(Message, Clone, Debug)]
#[rtype(usize)]
pub struct MeshPeerCount(pub TopicHash);

impl From<TopicHash> for MeshPeerCount {
    fn from(topic: TopicHash) -> Self {
        Self(topic)
    }
}

#[derive(Message, Clone, Debug)]
#[rtype("Vec<PeerId>")]
pub struct MeshPeers(pub TopicHash);

impl From<TopicHash> for MeshPeers {
    fn from(topic: TopicHash) -> Self {
        Self(topic)
    }
}

#[derive(Message, Clone, Copy, Debug)]
#[rtype("EyreResult<Stream>")]
pub struct OpenStream(pub PeerId);

impl From<PeerId> for OpenStream {
    fn from(peer_id: PeerId) -> Self {
        Self(peer_id)
    }
}

#[derive(Message, Clone, Copy, Debug)]
#[rtype(usize)]
pub struct PeerCount;

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<MessageId>")]
pub struct Publish {
    pub topic: TopicHash,
    pub data: Vec<u8>,
}

impl From<(TopicHash, Vec<u8>)> for Publish {
    fn from((topic, data): (TopicHash, Vec<u8>)) -> Self {
        Self { topic, data }
    }
}

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<IdentTopic>")]
pub struct Subscribe(pub IdentTopic);

impl From<IdentTopic> for Subscribe {
    fn from(topic: IdentTopic) -> Self {
        Self(topic)
    }
}

#[derive(Message, Clone, Debug)]
#[rtype("EyreResult<IdentTopic>")]
pub struct Unsubscribe(pub IdentTopic);

impl From<IdentTopic> for Unsubscribe {
    fn from(topic: IdentTopic) -> Self {
        Self(topic)
    }
}
