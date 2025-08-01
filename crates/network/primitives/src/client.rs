use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_utils_actix::LazyRecipient;
use libp2p::gossipsub::{IdentTopic, MessageId, TopicHash};
use libp2p::Multiaddr;
use tokio::sync::oneshot;

use crate::messages::{
    AnnounceBlob, Bootstrap, Dial, ListenOn, MeshPeerCount, MeshPeers, NetworkMessage, OpenStream,
    PeerCount, Publish, QueryBlob, RequestBlob, Subscribe, Unsubscribe,
};
use crate::stream::Stream;

#[derive(Clone, Debug)]
pub struct NetworkClient {
    network_manager: LazyRecipient<NetworkMessage>,
}

impl NetworkClient {
    pub const fn new(network_manager: LazyRecipient<NetworkMessage>) -> Self {
        Self { network_manager }
    }

    pub async fn dial(&self, peer_addr: Multiaddr) -> eyre::Result<()> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Dial {
                request: Dial(peer_addr),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn listen_on(&self, addr: Multiaddr) -> eyre::Result<()> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::ListenOn {
                request: ListenOn(addr),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn bootstrap(&self) -> eyre::Result<()> {
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

    pub async fn subscribe(&self, topic: IdentTopic) -> eyre::Result<IdentTopic> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Subscribe {
                request: Subscribe(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn unsubscribe(&self, topic: IdentTopic) -> eyre::Result<IdentTopic> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Unsubscribe {
                request: Unsubscribe(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn publish(&self, topic: TopicHash, data: Vec<u8>) -> eyre::Result<MessageId> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::Publish {
                request: Publish { topic, data },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn open_stream(&self, peer_id: libp2p::PeerId) -> eyre::Result<Stream> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::OpenStream {
                request: OpenStream(peer_id),
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
                request: MeshPeerCount(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    pub async fn mesh_peers(&self, topic: TopicHash) -> Vec<libp2p::PeerId> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::MeshPeers {
                request: MeshPeers(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    // Blob discovery methods

    /// Announce a blob to the DHT for a specific context
    pub async fn announce_blob(
        &self,
        blob_id: BlobId,
        context_id: ContextId,
        size: u64,
    ) -> eyre::Result<()> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::AnnounceBlob {
                request: AnnounceBlob {
                    blob_id,
                    context_id,
                    size,
                },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    /// Query the DHT for peers that have a specific blob
    pub async fn query_blob(
        &self,
        blob_id: BlobId,
        context_id: Option<ContextId>,
    ) -> eyre::Result<Vec<libp2p::PeerId>> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::QueryBlob {
                request: QueryBlob {
                    blob_id,
                    context_id,
                },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    /// Request a blob from a specific peer
    pub async fn request_blob(
        &self,
        blob_id: BlobId,
        context_id: ContextId,
        peer_id: libp2p::PeerId,
    ) -> eyre::Result<Option<Vec<u8>>> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::RequestBlob {
                request: RequestBlob {
                    blob_id,
                    context_id,
                    peer_id,
                },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }
}
