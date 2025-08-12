use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use libp2p::core::transport::ListenerId;
pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
pub use libp2p::PeerId;
use libp2p::{Multiaddr, StreamProtocol};
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
    // Blob discovery messages
    AnnounceBlob {
        request: AnnounceBlob,
        outcome: oneshot::Sender<<AnnounceBlob as actix::Message>::Result>,
    },
    QueryBlob {
        request: QueryBlob,
        outcome: oneshot::Sender<<QueryBlob as actix::Message>::Result>,
    },
    RequestBlob {
        request: RequestBlob,
        outcome: oneshot::Sender<<RequestBlob as actix::Message>::Result>,
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

// Blob discovery messages

/// Announce a blob to the DHT for a specific context
#[derive(Clone, Copy, Debug)]
pub struct AnnounceBlob {
    pub blob_id: BlobId,
    pub context_id: ContextId,
    pub size: u64,
}

impl actix::Message for AnnounceBlob {
    type Result = eyre::Result<()>;
}

/// Query for blob availability in the DHT
#[derive(Clone, Copy, Debug)]
pub struct QueryBlob {
    pub blob_id: BlobId,
    pub context_id: Option<ContextId>, // None for global queries
}

impl actix::Message for QueryBlob {
    type Result = eyre::Result<Vec<PeerId>>;
}

/// Request a blob from a specific peer
#[derive(Clone, Copy, Debug)]
pub struct RequestBlob {
    pub blob_id: BlobId,
    pub context_id: ContextId,
    pub peer_id: PeerId,
}

impl actix::Message for RequestBlob {
    type Result = eyre::Result<Option<Vec<u8>>>;
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
        protocol: StreamProtocol,
    },
    // Blob discovery events
    BlobRequested {
        blob_id: BlobId,
        context_id: ContextId,
        requesting_peer: PeerId,
    },
    BlobProvidersFound {
        blob_id: BlobId,
        context_id: Option<ContextId>,
        providers: Vec<PeerId>,
    },
    BlobDownloaded {
        blob_id: BlobId,
        context_id: ContextId,
        data: Vec<u8>,
        from_peer: PeerId,
    },
    BlobDownloadFailed {
        blob_id: BlobId,
        context_id: ContextId,
        from_peer: PeerId,
        error: String,
    },
}

impl actix::Message for NetworkEvent {
    type Result = ();
}
