use calimero_primitives::{blobs::BlobId, context::ContextId};
use calimero_utils_actix::LazyRecipient;
use libp2p::gossipsub::{IdentTopic, MessageId, PublishError, TopicHash};
use libp2p::request_response::{OutboundRequestId, ResponseChannel};
use libp2p::Multiaddr;
use tokio::sync::oneshot;

/// Returns true when `err`'s `eyre::Report` chain contains
/// `gossipsub::PublishError::NoPeersSubscribedToTopic`.
///
/// Single source of truth for classifying the one gossipsub publish error
/// callers handle differently from the rest: it's a normal cold-start
/// outcome (the topic has no subscribed peers yet), not a transport
/// failure. Callers that broadcast state deltas (`broadcast`,
/// `broadcast_heartbeat`) silence this variant and surface every other
/// error; callers that publish governance ops surface it explicitly via
/// `BroadcastPublishError::NoPeersSubscribed` so the higher-level
/// publish-with-acks flow can decide whether to wait for mesh formation.
///
/// Only walks `eyre::Report::chain()`. A `PublishError` wrapped in a
/// non-eyre `Box<dyn std::error::Error>` would not be detected here, but
/// every current caller goes through `NetworkClient::publish`, whose
/// return type is `eyre::Result<_>`, so the eyre chain is the only
/// shape that reaches this helper in practice.
#[must_use]
pub fn is_no_peers_subscribed_error(err: &eyre::Report) -> bool {
    err.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<PublishError>(),
            Some(PublishError::NoPeersSubscribedToTopic)
        )
    })
}

use crate::blob_types::BlobAuth;
use crate::messages::{
    AnnounceBlob, Bootstrap, Dial, ListenOn, MeshPeerCount, MeshPeers, MeshStats, NetworkMessage,
    NetworkStatus, OpenStream, PeerCount, Publish, QueryBlob, RequestBlob,
    SendSpecializedNodeInvitationResponse, SendSpecializedNodeVerificationRequest, Subscribe,
    SubscribedPeers, Unsubscribe,
};
use crate::network_status::NetworkStatusSnapshot;
use crate::specialized_node_invite::{SpecializedNodeInvitationResponse, VerificationRequest};
use crate::stream::Stream;

#[derive(Clone, Debug)]
pub struct NetworkClient {
    network_manager: LazyRecipient<NetworkMessage>,
}

impl NetworkClient {
    #[must_use]
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

        self.network_manager
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

    /// All connected peers subscribed to `topic` (the full subscriber
    /// set, not just the grafted mesh — see [`SubscribedPeers`]). Sync
    /// peer-selection uses this so it can reconcile with any connected
    /// subscriber regardless of mesh membership.
    pub async fn subscribed_peers(&self, topic: TopicHash) -> Vec<libp2p::PeerId> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::SubscribedPeers {
                request: SubscribedPeers(topic),
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    /// Per-topic mesh peer-count snapshot for every topic this node is
    /// subscribed to. Returns `(topic_hash, count)` pairs.
    pub async fn mesh_stats(&self) -> Vec<(TopicHash, usize)> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::MeshStats {
                request: MeshStats,
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    /// Snapshot of the local node's libp2p connectivity state. Feeds
    /// `GET /admin-api/network/status` and the corresponding
    /// `meroctl network status` CLI.
    pub async fn network_status(&self) -> NetworkStatusSnapshot {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::NetworkStatus {
                request: NetworkStatus,
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
        auth: Option<BlobAuth>,
    ) -> eyre::Result<Option<Vec<u8>>> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::RequestBlob {
                request: RequestBlob {
                    blob_id,
                    context_id,
                    peer_id,
                    auth,
                },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    // Specialized node invite protocol methods

    /// Send a specialized node verification request to a peer
    pub async fn send_specialized_node_verification_request(
        &self,
        peer_id: libp2p::PeerId,
        request: VerificationRequest,
    ) -> eyre::Result<OutboundRequestId> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::SendSpecializedNodeVerificationRequest {
                request: SendSpecializedNodeVerificationRequest { peer_id, request },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }

    /// Send a specialized node invitation response via the response channel
    pub async fn send_specialized_node_invitation_response(
        &self,
        channel: ResponseChannel<SpecializedNodeInvitationResponse>,
        response: SpecializedNodeInvitationResponse,
    ) -> eyre::Result<()> {
        let (tx, rx) = oneshot::channel();

        self.network_manager
            .send(NetworkMessage::SendSpecializedNodeInvitationResponse {
                request: SendSpecializedNodeInvitationResponse { channel, response },
                outcome: tx,
            })
            .await
            .expect("Mailbox not to be dropped");

        rx.await.expect("Mailbox not to be dropped")
    }
}

#[cfg(test)]
mod is_no_peers_subscribed_error_tests {
    use libp2p::gossipsub::PublishError;

    use super::is_no_peers_subscribed_error;

    #[test]
    fn classifies_no_peers_subscribed_at_root() {
        let err: eyre::Report = PublishError::NoPeersSubscribedToTopic.into();
        assert!(is_no_peers_subscribed_error(&err));
    }

    #[test]
    fn classifies_no_peers_subscribed_when_wrapped() {
        let err = eyre::Report::from(PublishError::NoPeersSubscribedToTopic)
            .wrap_err("failed to publish state delta");
        assert!(is_no_peers_subscribed_error(&err));
    }

    #[test]
    fn does_not_classify_other_publish_errors() {
        let err: eyre::Report = PublishError::MessageTooLarge.into();
        assert!(!is_no_peers_subscribed_error(&err));
    }

    #[test]
    fn does_not_classify_unrelated_errors() {
        let err: eyre::Report = eyre::eyre!("some other failure");
        assert!(!is_no_peers_subscribed_error(&err));
    }
}
