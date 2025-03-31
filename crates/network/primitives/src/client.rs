use actix::Message;
use calimero_utils_actix::LazyRecipient;
use eyre::Result as EyreResult;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::{Multiaddr, PeerId};
use tokio::sync::oneshot;

use crate::messages::{
    Bootstrap, Dial, ListenOn, MeshPeerCount, MeshPeers, OpenStream, PeerCount, Publish, Subscribe,
    Unsubscribe,
};
use crate::stream::Stream;

#[derive(Clone, Debug)]
pub struct NetworkClient {
    network_manager: LazyRecipient<NetworkMessage>,
}

#[derive(Debug, Message)]
#[rtype("()")]
pub enum NetworkMessage {
    Dial {
        request: Dial,
        outcome: oneshot::Sender<EyreResult<()>>,
    },
    ListenOn {
        request: ListenOn,
        outcome: oneshot::Sender<EyreResult<()>>,
    },
    Bootstrap {
        request: Bootstrap,
        outcome: oneshot::Sender<EyreResult<()>>,
    },
    Subscribe {
        request: Subscribe,
        outcome: oneshot::Sender<EyreResult<IdentTopic>>,
    },
    Unsubscribe {
        request: Unsubscribe,
        outcome: oneshot::Sender<EyreResult<IdentTopic>>,
    },
    Publish {
        request: Publish,
        outcome: oneshot::Sender<EyreResult<MessageId>>,
    },
    OpenStream {
        request: OpenStream,
        outcome: oneshot::Sender<EyreResult<Stream>>,
    },
    PeerCount {
        request: PeerCount,
        outcome: oneshot::Sender<usize>,
    },
    MeshPeers {
        request: MeshPeers,
        outcome: oneshot::Sender<Vec<PeerId>>,
    },
    MeshPeerCount {
        request: MeshPeerCount,
        outcome: oneshot::Sender<usize>,
    },
}

impl NetworkClient {
    pub const fn new(network_manager: LazyRecipient<NetworkMessage>) -> Self {
        Self { network_manager }
    }

    pub async fn dial(&self, peer_addr: Multiaddr) -> EyreResult<()> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Dial {
                request: Dial::from(peer_addr),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn listen_on(&self, addr: Multiaddr) -> EyreResult<()> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::ListenOn {
                request: ListenOn::from(addr),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn bootstrap(&self) -> EyreResult<()> {
        let (tx, rx) = oneshot::channel();

        let _result = self
            .network_manager
            .send(NetworkMessage::Bootstrap {
                request: Bootstrap,
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn subscribe(&self, topic: IdentTopic) -> EyreResult<IdentTopic> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Subscribe {
                request: Subscribe::from(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn unsubscribe(&self, topic: IdentTopic) -> EyreResult<IdentTopic> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Unsubscribe {
                request: Unsubscribe::from(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn publish(&self, topic: TopicHash, data: Vec<u8>) -> EyreResult<MessageId> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Publish {
                request: Publish::from((topic, data)),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn open_stream(&self, peer_id: PeerId) -> EyreResult<Stream> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::OpenStream {
                request: OpenStream::from(peer_id),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn peer_count(&self) -> usize {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::PeerCount {
                request: PeerCount,
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn mesh_peer_count(&self, topic: TopicHash) -> usize {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::MeshPeerCount {
                request: MeshPeerCount::from(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn mesh_peers(&self, topic: TopicHash) -> Vec<PeerId> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::MeshPeers {
                request: MeshPeers::from(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }
}
