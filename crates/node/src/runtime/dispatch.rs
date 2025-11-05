//! Protocol Dispatch - Message types for the runtime
//!
//! Defines the messages that flow through the event loop.

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_storage::logical_clock::HybridTimestamp;
use libp2p::PeerId;

/// Gossipsub broadcast message (state deltas)
#[derive(Debug)]
pub enum GossipsubMessage {
    /// State delta broadcast from peer
    StateDelta {
        /// Source peer
        source: PeerId,
        
        /// Context ID
        context_id: ContextId,
        
        /// Delta author
        author_id: PublicKey,
        
        /// Delta ID
        delta_id: [u8; 32],
        
        /// Parent delta IDs
        parent_ids: Vec<[u8; 32]>,
        
        /// Hybrid logical clock timestamp
        hlc: HybridTimestamp,
        
        /// Expected root hash after applying
        root_hash: Hash,
        
        /// Encrypted delta payload
        artifact: Vec<u8>,
        
        /// Nonce for decryption
        nonce: Nonce,
        
        /// Optional serialized events
        events: Option<Vec<u8>>,
    },
}

/// P2P request/response message
#[derive(Debug)]
pub enum P2pRequest {
    /// Delta request from peer
    DeltaRequest {
        /// Stream for communication
        stream: Stream,
        
        /// Context ID
        context_id: ContextId,
        
        /// Delta ID being requested
        delta_id: [u8; 32],
        
        /// Peer's identity
        their_identity: PublicKey,
        
        /// Our identity
        our_identity: PublicKey,
    },
    
    /// Blob request from peer
    BlobRequest {
        /// Stream for communication
        stream: Stream,
        
        /// Context for the blob
        context: Context,
        
        /// Our identity
        our_identity: PublicKey,
        
        /// Peer's identity
        their_identity: PublicKey,
        
        /// Blob ID being requested
        blob_id: BlobId,
    },
    
    /// Key exchange request from peer
    KeyExchange {
        /// Stream for communication
        stream: Stream,
        
        /// Context for the key exchange
        context: Context,
        
        /// Our identity
        our_identity: PublicKey,
        
        /// Peer's identity
        their_identity: PublicKey,
        
        /// Peer's nonce
        their_nonce: Nonce,
    },
}

/// Sync request (from API or internal triggers)
#[derive(Debug)]
pub struct SyncRequest {
    /// Context to sync
    pub context_id: ContextId,
    
    /// Optional specific peer to sync with
    pub peer_id: Option<PeerId>,
    
    /// Optional channel for result
    pub result_tx: Option<tokio::sync::oneshot::Sender<calimero_sync::strategies::SyncResult>>,
}

