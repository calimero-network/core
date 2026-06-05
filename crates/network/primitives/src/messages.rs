//! Network message types and event dispatching.
//!
//! This module defines the message types used for communication between the
//! [`NetworkClient`](crate::client::NetworkClient) and the `NetworkManager` actor,
//! as well as the events emitted by the network layer.
//!
//! # Architecture
//!
//! ```text
//! Application Layer (calimero-node)
//!        │
//!        │ NetworkMessage (commands)
//!        ▼
//! ┌─────────────────────────────────────┐
//! │         NetworkManager              │
//! │         (actix actor)               │
//! └─────────────────────────────────────┘
//!        │
//!        │ NetworkEvent (events)
//!        ▼
//! Application Layer (via NetworkEventDispatcher)
//! ```
//!
//! # Message Types
//!
//! - [`NetworkMessage`]: Commands sent to `NetworkManager` (subscribe, publish, etc.)
//! - [`NetworkEvent`]: Events emitted by `NetworkManager` (messages received, streams opened, etc.)
//!
//! # Event Dispatching
//!
//! The [`NetworkEventDispatcher`] trait allows different mechanisms for delivering
//! events from the network layer to the application:
//!
//! ```ignore
//! // Example implementation using a channel
//! struct ChannelDispatcher {
//!     tx: mpsc::Sender<NetworkEvent>,
//! }
//!
//! impl NetworkEventDispatcher for ChannelDispatcher {
//!     fn dispatch(&self, event: NetworkEvent) -> bool {
//!         self.tx.try_send(event).is_ok()
//!     }
//! }
//! ```

use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use libp2p::core::transport::ListenerId;
pub use libp2p::gossipsub::{IdentTopic, Message, MessageId, TopicHash};
pub use libp2p::request_response::{InboundRequestId, OutboundRequestId, ResponseChannel};
pub use libp2p::PeerId;
use libp2p::{Multiaddr, StreamProtocol};
use tokio::sync::oneshot;

use crate::blob_types::BlobAuth;
use crate::network_status::NetworkStatusSnapshot;
use crate::specialized_node_invite::{SpecializedNodeInvitationResponse, VerificationRequest};
use crate::stream::Stream;

/// Commands sent to the `NetworkManager` actor.
///
/// Each variant wraps a specific request type and includes a oneshot channel
/// for returning the result. The `NetworkClient` constructs these messages
/// and awaits the result via the oneshot receiver.
///
/// # Example
///
/// ```ignore
/// // This is how NetworkClient sends a command internally:
/// let (tx, rx) = oneshot::channel();
/// network_manager.send(NetworkMessage::Subscribe {
///     request: Subscribe(topic),
///     outcome: tx,
/// }).await?;
/// let result = rx.await?;
/// ```
#[derive(Debug, actix::Message)]
#[rtype("()")]
pub enum NetworkMessage {
    /// Dial a peer at the specified multiaddress.
    Dial {
        request: Dial,
        outcome: oneshot::Sender<<Dial as actix::Message>::Result>,
    },
    /// Start listening on a new address.
    ListenOn {
        request: ListenOn,
        outcome: oneshot::Sender<<ListenOn as actix::Message>::Result>,
    },
    /// Bootstrap the Kademlia DHT.
    Bootstrap {
        request: Bootstrap,
        outcome: oneshot::Sender<<Bootstrap as actix::Message>::Result>,
    },
    /// Subscribe to a gossipsub topic.
    Subscribe {
        request: Subscribe,
        outcome: oneshot::Sender<<Subscribe as actix::Message>::Result>,
    },
    /// Unsubscribe from a gossipsub topic.
    Unsubscribe {
        request: Unsubscribe,
        outcome: oneshot::Sender<<Unsubscribe as actix::Message>::Result>,
    },
    /// Publish a message to a gossipsub topic.
    Publish {
        request: Publish,
        outcome: oneshot::Sender<<Publish as actix::Message>::Result>,
    },
    /// Open a direct stream to a peer.
    OpenStream {
        request: OpenStream,
        outcome: oneshot::Sender<<OpenStream as actix::Message>::Result>,
    },
    /// Get the count of connected peers.
    PeerCount {
        request: PeerCount,
        outcome: oneshot::Sender<<PeerCount as actix::Message>::Result>,
    },
    /// Get the list of mesh peers for a topic.
    MeshPeers {
        request: MeshPeers,
        outcome: oneshot::Sender<<MeshPeers as actix::Message>::Result>,
    },
    /// Get all connected peers SUBSCRIBED to a topic (the full
    /// subscriber set, not just the grafted mesh).
    SubscribedPeers {
        request: SubscribedPeers,
        outcome: oneshot::Sender<<SubscribedPeers as actix::Message>::Result>,
    },
    /// Get the count of mesh peers for a topic.
    MeshPeerCount {
        request: MeshPeerCount,
        outcome: oneshot::Sender<<MeshPeerCount as actix::Message>::Result>,
    },
    /// Get the per-topic mesh peer-count snapshot for every topic this
    /// node is subscribed to.
    MeshStats {
        request: MeshStats,
        outcome: oneshot::Sender<<MeshStats as actix::Message>::Result>,
    },
    /// Snapshot the swarm connectivity state for `GET /admin-api/network/status`.
    NetworkStatus {
        request: NetworkStatus,
        outcome: oneshot::Sender<<NetworkStatus as actix::Message>::Result>,
    },
    /// Announce blob availability to the DHT.
    AnnounceBlob {
        request: AnnounceBlob,
        outcome: oneshot::Sender<<AnnounceBlob as actix::Message>::Result>,
    },
    /// Query the DHT for blob providers.
    QueryBlob {
        request: QueryBlob,
        outcome: oneshot::Sender<<QueryBlob as actix::Message>::Result>,
    },
    /// Request a blob from a specific peer.
    RequestBlob {
        request: RequestBlob,
        outcome: oneshot::Sender<<RequestBlob as actix::Message>::Result>,
    },
    /// Send a specialized node verification request.
    SendSpecializedNodeVerificationRequest {
        request: SendSpecializedNodeVerificationRequest,
        outcome:
            oneshot::Sender<<SendSpecializedNodeVerificationRequest as actix::Message>::Result>,
    },
    /// Send a specialized node invitation response.
    SendSpecializedNodeInvitationResponse {
        request: SendSpecializedNodeInvitationResponse,
        outcome: oneshot::Sender<<SendSpecializedNodeInvitationResponse as actix::Message>::Result>,
    },
    /// Set a peer's gossipsub application-specific score (membership bias).
    SetPeerScore {
        request: SetPeerScore,
        outcome: oneshot::Sender<<SetPeerScore as actix::Message>::Result>,
    },
}

/// Request to bootstrap the Kademlia DHT.
///
/// This initiates the DHT bootstrap process, connecting to bootstrap nodes
/// and populating the routing table.
#[derive(Clone, Copy, Debug)]
pub struct Bootstrap;

impl actix::Message for Bootstrap {
    type Result = eyre::Result<()>;
}

/// Request to dial a peer at a specific multiaddress.
///
/// # Example
///
/// ```ignore
/// let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001/p2p/QmPeer...".parse()?;
/// network_client.dial(addr).await?;
/// ```
#[derive(Clone, Debug)]
pub struct Dial(pub Multiaddr);

impl actix::Message for Dial {
    type Result = eyre::Result<()>;
}

/// Request to start listening on a new address.
///
/// The network manager will begin accepting connections on the specified address.
///
/// # Example
///
/// ```ignore
/// let addr: Multiaddr = "/ip4/0.0.0.0/tcp/4001".parse()?;
/// network_client.listen_on(addr).await?;
/// ```
#[derive(Clone, Debug)]
pub struct ListenOn(pub Multiaddr);

impl actix::Message for ListenOn {
    type Result = eyre::Result<()>;
}

/// Request to get the count of mesh peers for a gossipsub topic.
///
/// Mesh peers are the subset of topic subscribers that are directly
/// connected in the gossipsub mesh for efficient message propagation.
#[derive(Clone, Debug)]
pub struct MeshPeerCount(pub TopicHash);

impl actix::Message for MeshPeerCount {
    type Result = usize;
}

/// Request to get the list of mesh peers for a gossipsub topic.
///
/// Returns the peer IDs of all peers in the gossipsub mesh for this topic.
#[derive(Clone, Debug)]
pub struct MeshPeers(pub TopicHash);

impl actix::Message for MeshPeers {
    type Result = Vec<PeerId>;
}

/// Returns the peer IDs of all CONNECTED peers subscribed to this topic —
/// the full subscriber set from gossipsub's peer-topics table, populated
/// on `SUBSCRIBE` (no GRAFT required). Distinct from [`MeshPeers`], which
/// returns only the bounded grafted mesh: a peer can be connected and
/// subscribed yet not (yet/still) in the mesh. Sync peer-selection uses
/// this so it can reconcile with any subscribed peer it's connected to,
/// independent of mesh health.
#[derive(Clone, Debug)]
pub struct SubscribedPeers(pub TopicHash);

impl actix::Message for SubscribedPeers {
    type Result = Vec<PeerId>;
}

/// Request a per-topic mesh peer-count snapshot covering every topic
/// this node is currently subscribed to.
///
/// Returns `(topic_hash, mesh_peer_count)` pairs. Intended for periodic
/// CI-observable logging so the actual mesh state is visible alongside
/// the libp2p-gossipsub internal logs (which report mesh additions per
/// heartbeat, not current mesh size, and are easy to misread).
#[derive(Clone, Copy, Debug)]
pub struct MeshStats;

impl actix::Message for MeshStats {
    type Result = Vec<(TopicHash, usize)>;
}

/// Request a snapshot of the local node's libp2p connectivity state —
/// listen addresses, external addresses, relay reservations,
/// rendezvous registrations, DCUtR upgrade outcomes per peer, and the
/// most recent AutoNAT v2 probe result. Used by
/// `GET /admin-api/network/status` and by integration tests that want
/// typed assertions instead of grepping logs.
#[derive(Clone, Copy, Debug)]
pub struct NetworkStatus;

impl actix::Message for NetworkStatus {
    type Result = NetworkStatusSnapshot;
}

/// Request to open a direct stream to a peer.
///
/// Opens a bidirectional stream using the Calimero stream protocol
/// (`/calimero/stream/0.0.2`). The returned [`Stream`] can be used
/// for sending and receiving framed messages.
///
/// # Example
///
/// ```ignore
/// let stream = network_client.open_stream(peer_id).await?;
/// stream.send(Message::new(data)).await?;
/// let response = stream.recv().await?;
/// ```
#[derive(Clone, Copy, Debug)]
pub struct OpenStream(pub PeerId);

impl actix::Message for OpenStream {
    type Result = eyre::Result<Stream>;
}

/// Request to get the count of connected peers.
///
/// Returns the total number of peers currently connected to this node.
#[derive(Clone, Copy, Debug)]
pub struct PeerCount;

impl actix::Message for PeerCount {
    type Result = usize;
}

/// Request to publish a message to a gossipsub topic.
///
/// The message will be broadcast to all peers subscribed to the topic
/// via the gossipsub mesh.
///
/// # Fields
///
/// * `topic` - The topic hash to publish to (typically `context_id.to_string().hash()`)
/// * `data` - The message payload (typically serialized state delta)
#[derive(Clone, Debug)]
pub struct Publish {
    /// The gossipsub topic hash.
    pub topic: TopicHash,
    /// The message data to publish.
    pub data: Vec<u8>,
}

impl actix::Message for Publish {
    type Result = eyre::Result<MessageId>;
}

/// Request to subscribe to a gossipsub topic.
///
/// After subscribing, the node will receive [`NetworkEvent::Message`] events
/// for messages published to this topic.
///
/// # Example
///
/// ```ignore
/// let topic = IdentTopic::new(context_id.to_string());
/// network_client.subscribe(topic).await?;
/// ```
#[derive(Clone, Debug)]
pub struct Subscribe(pub IdentTopic);

impl actix::Message for Subscribe {
    type Result = eyre::Result<IdentTopic>;
}

/// Set a peer's gossipsub application-specific score (#2513). Pushed by
/// the node when it (de)verifies a peer's membership; the network layer
/// applies it via `gossipsub::Behaviour::set_application_score`.
#[derive(Debug, Clone, Copy)]
pub struct SetPeerScore {
    pub peer_id: PeerId,
    pub score: f64,
}

impl actix::Message for SetPeerScore {
    type Result = ();
}

/// Request to unsubscribe from a gossipsub topic.
///
/// After unsubscribing, the node will no longer receive messages for this topic.
#[derive(Clone, Debug)]
pub struct Unsubscribe(pub IdentTopic);

impl actix::Message for Unsubscribe {
    type Result = eyre::Result<IdentTopic>;
}

// ============================================================================
// Blob Discovery Messages
// ============================================================================

/// Request to announce blob availability to the DHT.
///
/// This registers the local node as a provider for the specified blob,
/// allowing other nodes to discover and request it.
///
/// # Fields
///
/// * `blob_id` - The unique identifier of the blob (typically content hash)
/// * `context_id` - The context this blob belongs to
/// * `size` - The size of the blob in bytes
#[derive(Clone, Copy, Debug)]
pub struct AnnounceBlob {
    /// The blob identifier.
    pub blob_id: BlobId,
    /// The context this blob is associated with.
    pub context_id: ContextId,
    /// The size of the blob in bytes.
    pub size: u64,
}

impl actix::Message for AnnounceBlob {
    type Result = eyre::Result<()>;
}

/// Request to query the DHT for peers that have a specific blob.
///
/// Returns a list of peer IDs that have announced availability of this blob.
///
/// # Fields
///
/// * `blob_id` - The blob to search for
/// * `context_id` - Optional context filter (None for global queries)
#[derive(Clone, Copy, Debug)]
pub struct QueryBlob {
    /// The blob identifier to query for.
    pub blob_id: BlobId,
    /// Optional context filter. If `None`, queries globally.
    pub context_id: Option<ContextId>,
}

impl actix::Message for QueryBlob {
    type Result = eyre::Result<Vec<PeerId>>;
}

/// Request to download a blob from a specific peer.
///
/// Opens a stream to the peer and requests the blob data.
///
/// # Fields
///
/// * `blob_id` - The blob to request
/// * `context_id` - The context for authorization
/// * `peer_id` - The peer to request from (typically from [`QueryBlob`] results)
/// * `auth` - Optional authentication data
#[derive(Clone, Copy, Debug)]
pub struct RequestBlob {
    /// The blob identifier to request.
    pub blob_id: BlobId,
    /// The context for authorization.
    pub context_id: ContextId,
    /// The peer to request the blob from.
    pub peer_id: PeerId,
    /// Optional authentication data.
    pub auth: Option<BlobAuth>,
}

impl actix::Message for RequestBlob {
    type Result = eyre::Result<Option<Vec<u8>>>;
}

// ============================================================================
// Specialized Node Invite Protocol Messages
// ============================================================================

/// Request to send a verification request to a peer.
///
/// Used in the specialized node invitation protocol where a new node
/// verifies its identity with an existing node.
#[derive(Debug)]
pub struct SendSpecializedNodeVerificationRequest {
    /// The peer to send the verification request to.
    pub peer_id: PeerId,
    /// The verification request payload.
    pub request: VerificationRequest,
}

impl actix::Message for SendSpecializedNodeVerificationRequest {
    type Result = eyre::Result<OutboundRequestId>;
}

/// Request to send an invitation response via a response channel.
///
/// Used to respond to an incoming verification request in the
/// specialized node invitation protocol.
#[derive(Debug)]
pub struct SendSpecializedNodeInvitationResponse {
    /// The response channel from the incoming request.
    pub channel: ResponseChannel<SpecializedNodeInvitationResponse>,
    /// The response to send.
    pub response: SpecializedNodeInvitationResponse,
}

impl actix::Message for SendSpecializedNodeInvitationResponse {
    type Result = eyre::Result<()>;
}

// ============================================================================
// Network Events
// ============================================================================

/// Events emitted by the network layer.
///
/// These events are dispatched via the [`NetworkEventDispatcher`] to notify
/// the application layer of network activity.
///
/// # Event Types
///
/// | Event | Description |
/// |-------|-------------|
/// | [`ListeningOn`](NetworkEvent::ListeningOn) | Node started listening on an address |
/// | [`Subscribed`](NetworkEvent::Subscribed) | A peer subscribed to a topic |
/// | [`Unsubscribed`](NetworkEvent::Unsubscribed) | A peer unsubscribed from a topic |
/// | [`Message`](NetworkEvent::Message) | Received a gossipsub message |
/// | [`StreamOpened`](NetworkEvent::StreamOpened) | A peer opened a stream to us |
/// | `Blob*` events | Blob discovery and transfer events |
/// | `SpecializedNode*` events | Node invitation protocol events |
///
/// # Example Handler
///
/// ```ignore
/// impl NetworkEventDispatcher for MyHandler {
///     fn dispatch(&self, event: NetworkEvent) -> bool {
///         match event {
///             NetworkEvent::Message { id, message } => {
///                 // Process incoming gossipsub message
///                 self.handle_message(message);
///             }
///             NetworkEvent::StreamOpened { peer_id, stream, protocol } => {
///                 // Handle incoming stream
///                 self.handle_stream(peer_id, stream);
///             }
///             _ => {}
///         }
///         true
///     }
/// }
/// ```
#[derive(Debug)]
pub enum NetworkEvent {
    /// The node started listening on a new address.
    ///
    /// Emitted when `listen_on()` succeeds and the address is confirmed.
    ListeningOn {
        /// The listener ID for this address.
        listener_id: ListenerId,
        /// The address being listened on (includes `/p2p/<peer_id>`).
        address: Multiaddr,
    },

    /// A remote peer subscribed to a gossipsub topic.
    ///
    /// Useful for tracking which peers are interested in a context.
    Subscribed {
        /// The peer that subscribed.
        peer_id: PeerId,
        /// The topic they subscribed to.
        topic: TopicHash,
    },

    /// A remote peer unsubscribed from a gossipsub topic.
    Unsubscribed {
        /// The peer that unsubscribed.
        peer_id: PeerId,
        /// The topic they unsubscribed from.
        topic: TopicHash,
    },

    /// Received a message on a subscribed gossipsub topic.
    ///
    /// This is the primary event for receiving state deltas and other
    /// broadcast messages from context members.
    Message {
        /// Unique identifier for this message.
        id: MessageId,
        /// The gossipsub message containing topic, data, and sender info.
        message: Message,
    },

    /// A remote peer opened a stream to this node.
    ///
    /// The stream can be used for bidirectional communication.
    /// Common protocols:
    /// - `/calimero/stream/0.0.2` - General sync streams
    /// - `/calimero/blob/0.0.2` - Blob transfers
    StreamOpened {
        /// The peer that opened the stream.
        peer_id: PeerId,
        /// The bidirectional stream (boxed for size).
        stream: Box<Stream>,
        /// The protocol negotiated for this stream.
        protocol: StreamProtocol,
    },

    /// A peer requested a blob from us.
    BlobRequested {
        /// The blob being requested.
        blob_id: BlobId,
        /// The context for authorization.
        context_id: ContextId,
        /// The peer requesting the blob.
        requesting_peer: PeerId,
    },

    /// DHT query found providers for a blob.
    BlobProvidersFound {
        /// The blob that was queried.
        blob_id: BlobId,
        /// The context filter used (if any).
        context_id: Option<ContextId>,
        /// List of peers that have this blob.
        providers: Vec<PeerId>,
    },

    /// Successfully downloaded a blob from a peer.
    BlobDownloaded {
        /// The blob that was downloaded.
        blob_id: BlobId,
        /// The context this blob belongs to.
        context_id: ContextId,
        /// The blob data.
        data: Vec<u8>,
        /// The peer we downloaded from.
        from_peer: PeerId,
    },

    /// Failed to download a blob from a peer.
    BlobDownloadFailed {
        /// The blob that failed to download.
        blob_id: BlobId,
        /// The context this blob belongs to.
        context_id: ContextId,
        /// The peer we tried to download from.
        from_peer: PeerId,
        /// Error description.
        error: String,
    },

    /// Received a verification request from a specialized node.
    ///
    /// The application should verify the request and send a response
    /// via the provided channel.
    SpecializedNodeVerificationRequest {
        /// The peer sending the verification request.
        peer_id: PeerId,
        /// Request ID for correlation.
        request_id: InboundRequestId,
        /// The verification request payload.
        request: VerificationRequest,
        /// Channel to send the response.
        channel: ResponseChannel<SpecializedNodeInvitationResponse>,
    },

    /// Received an invitation response from a peer.
    SpecializedNodeInvitationResponse {
        /// The peer that sent the response.
        peer_id: PeerId,
        /// Request ID for correlation with the original request.
        request_id: OutboundRequestId,
        /// The invitation response.
        response: SpecializedNodeInvitationResponse,
    },
}

impl actix::Message for NetworkEvent {
    type Result = ();
}

// ============================================================================
// Event Dispatching
// ============================================================================

/// Trait for dispatching network events to the application layer.
///
/// This trait enables flexible event delivery mechanisms. The `NetworkManager`
/// holds an `Arc<dyn NetworkEventDispatcher>` and calls `dispatch()` for each
/// network event.
///
/// # Implementations
///
/// Common implementations include:
/// - Channel-based: Send events through an `mpsc` channel
/// - Actor-based: Send to an Actix actor via `Recipient`
/// - Direct: Call handler methods directly
///
/// # Return Value
///
/// The `dispatch` method returns `true` if the event was successfully delivered,
/// `false` if it was dropped (e.g., channel full, receiver gone). Dropped events
/// are logged as warnings by the network layer.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use tokio::sync::mpsc;
///
/// struct ChannelDispatcher {
///     tx: mpsc::Sender<NetworkEvent>,
/// }
///
/// impl NetworkEventDispatcher for ChannelDispatcher {
///     fn dispatch(&self, event: NetworkEvent) -> bool {
///         // Use try_send to avoid blocking
///         self.tx.try_send(event).is_ok()
///     }
/// }
///
/// // Usage
/// let (tx, mut rx) = mpsc::channel(100);
/// let dispatcher: Arc<dyn NetworkEventDispatcher> = Arc::new(ChannelDispatcher { tx });
///
/// // NetworkManager will call dispatcher.dispatch(event) for each event
/// // Application receives events from rx
/// while let Some(event) = rx.recv().await {
///     handle_event(event);
/// }
/// ```
pub trait NetworkEventDispatcher: Send + Sync {
    /// Dispatch a network event.
    ///
    /// # Arguments
    ///
    /// * `event` - The network event to dispatch
    ///
    /// # Returns
    ///
    /// * `true` - Event was successfully dispatched
    /// * `false` - Event was dropped (logged as warning)
    fn dispatch(&self, event: NetworkEvent) -> bool;
}

/// Type alias for a boxed event dispatcher.
///
/// Useful when you need owned dispatch capability without `Arc`.
pub type BoxedEventDispatcher = Box<dyn NetworkEventDispatcher>;
