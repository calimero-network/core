//! Wire protocol types for sync stream communication.
//!
//! This module contains the message types used for all sync protocol
//! communication over network streams:
//!
//! - [`StreamMessage`]: Top-level message wrapper (Init or Message)
//! - [`InitPayload`]: Initial request types (blob share, key share, delta, snapshot, etc.)
//! - [`MessagePayload`]: Response and follow-up message types
//!
//! # Protocol Flow
//!
//! ```text
//! Initiator                              Responder
//! │                                            │
//! │ ── StreamMessage::Init { payload } ──────► │
//! │                                            │
//! │ ◄── StreamMessage::Message { payload } ── │
//! │                                            │
//! │ ... (continue as needed) ...               │
//! └────────────────────────────────────────────┘
//! ```
//!
//! # Adding New Protocols
//!
//! To add a new sync protocol's wire messages:
//! 1. Add request variant to [`InitPayload`]
//! 2. Add response variant(s) to [`MessagePayload`]
//! 3. Update re-exports in `sync.rs`

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};

use super::hash_comparison::TreeNode;
use super::snapshot::SnapshotError;

/// Maximum depth allowed in TreeNodeRequest.
///
/// Prevents malicious peers from requesting expensive deep traversals.
/// Handlers should validate against this limit before processing.
pub const MAX_TREE_REQUEST_DEPTH: u8 = 16;

// =============================================================================
// Stream Message Wrapper
// =============================================================================

/// Top-level message for sync stream communication.
///
/// All sync protocol messages are wrapped in this enum, which provides:
/// - Context and identity information (in Init)
/// - Sequence tracking (in Message)
/// - Nonce for replay protection
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    /// Initial message to start a sync operation.
    Init {
        /// Context being synchronized.
        context_id: ContextId,
        /// Identity of the sending party.
        party_id: PublicKey,
        /// The specific request payload.
        payload: InitPayload,
        /// Nonce for the next message.
        next_nonce: Nonce,
    },
    /// Follow-up message in an ongoing sync operation.
    Message {
        /// Sequence number for ordering.
        ///
        /// # Wire Format Change
        ///
        /// Changed from `usize` to `u64` for cross-platform portability.
        /// This is a breaking wire format change - nodes must be upgraded
        /// together to avoid deserialization failures.
        sequence_id: u64,
        /// The message payload.
        payload: MessagePayload<'a>,
        /// Nonce for the next message.
        next_nonce: Nonce,
    },
    /// Opaque error - reveals nothing about node state.
    ///
    /// Used when something goes wrong but we don't want to leak
    /// information to potentially malicious peers.
    OpaqueError,
}

// =============================================================================
// Init Payload (Requests)
// =============================================================================

/// Initial request payloads for various sync protocols.
///
/// Each variant represents a different type of sync request that can
/// be initiated by a node.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum InitPayload {
    /// Request to share a blob.
    BlobShare {
        /// ID of the blob to share.
        blob_id: BlobId,
    },

    /// Request to share encryption keys.
    KeyShare,

    /// Request a specific delta by ID (for DAG gap filling).
    DeltaRequest {
        /// Context for the delta.
        context_id: ContextId,
        /// ID of the specific delta to request.
        delta_id: [u8; 32],
    },

    /// Request peer's current DAG heads for catchup.
    DagHeadsRequest {
        /// Context to get DAG heads for.
        context_id: ContextId,
    },

    /// Request snapshot boundary negotiation.
    SnapshotBoundaryRequest {
        /// Context for snapshot sync.
        context_id: ContextId,
        /// Optional requested cutoff timestamp.
        requested_cutoff_timestamp: Option<u64>,
    },

    /// Request to stream snapshot pages.
    SnapshotStreamRequest {
        /// Context for snapshot sync.
        context_id: ContextId,
        /// Root hash that was negotiated in boundary request.
        boundary_root_hash: Hash,
        /// Maximum pages per response.
        page_limit: u16,
        /// Maximum bytes per response.
        byte_limit: u32,
        /// Resume cursor from previous page (for pagination).
        resume_cursor: Option<Vec<u8>>,
    },

    /// Request tree node(s) for HashComparison sync (CIP §4).
    ///
    /// Used by the HashComparison protocol to request subtrees from a peer
    /// for Merkle tree comparison.
    TreeNodeRequest {
        /// Context being synchronized.
        context_id: ContextId,
        /// ID of the node to request (root hash or entity ID).
        node_id: [u8; 32],
        /// Maximum depth to traverse from this node.
        /// None means only the requested node, Some(1) includes immediate children.
        max_depth: Option<u8>,
    },
}

// =============================================================================
// Message Payload (Responses)
// =============================================================================

/// Response and follow-up message payloads.
///
/// Each variant represents a different type of response or continuation
/// message in a sync protocol exchange.
#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    /// Blob data chunk.
    BlobShare {
        /// Chunk of blob data.
        chunk: Cow<'a, [u8]>,
    },

    /// Encryption key share.
    KeyShare {
        /// The sender's private key for the context.
        sender_key: PrivateKey,
    },

    /// Response to DeltaRequest containing the requested delta.
    DeltaResponse {
        /// The serialized delta data.
        delta: Cow<'a, [u8]>,
    },

    /// Delta not found response.
    DeltaNotFound,

    /// Response to DagHeadsRequest containing peer's current heads and root hash.
    DagHeadsResponse {
        /// Current DAG head hashes.
        dag_heads: Vec<[u8; 32]>,
        /// Current root hash.
        root_hash: Hash,
    },

    /// Challenge to prove ownership of claimed identity.
    Challenge {
        /// Random challenge bytes.
        challenge: [u8; 32],
    },

    /// Response to challenge with signature (Ed25519 signature is 64 bytes).
    ChallengeResponse {
        /// Signature proving identity ownership.
        signature: [u8; 64],
    },

    /// Response to SnapshotBoundaryRequest.
    SnapshotBoundaryResponse {
        /// Authoritative boundary timestamp (nanoseconds since epoch).
        boundary_timestamp: u64,
        /// Root hash for the boundary state.
        boundary_root_hash: Hash,
        /// Peer's DAG heads at the boundary.
        dag_heads: Vec<[u8; 32]>,
    },

    /// A page of snapshot data.
    SnapshotPage {
        /// Compressed payload data.
        payload: Cow<'a, [u8]>,
        /// Uncompressed length for validation.
        uncompressed_len: u32,
        /// Cursor for resuming (None if complete).
        cursor: Option<Vec<u8>>,
        /// Total page count.
        page_count: u64,
        /// Pages sent so far.
        sent_count: u64,
    },

    /// Snapshot sync error.
    SnapshotError {
        /// The error that occurred.
        error: SnapshotError,
    },

    /// Response to TreeNodeRequest for HashComparison sync (CIP §4).
    ///
    /// Contains tree nodes from the requested subtree for Merkle comparison.
    TreeNodeResponse {
        /// Tree nodes in the requested subtree.
        ///
        /// For a request with max_depth=0: contains just the requested node.
        /// For max_depth=1: contains the node and its immediate children.
        nodes: Vec<TreeNode>,
        /// True if the requested node was not found.
        not_found: bool,
    },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_payload_tree_node_request() {
        let request = InitPayload::TreeNodeRequest {
            context_id: ContextId::from([1u8; 32]),
            node_id: [2u8; 32],
            max_depth: Some(1),
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: InitPayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            InitPayload::TreeNodeRequest {
                context_id,
                node_id,
                max_depth,
            } => {
                assert_eq!(*context_id.as_ref(), [1u8; 32]);
                assert_eq!(node_id, [2u8; 32]);
                assert_eq!(max_depth, Some(1));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_tree_node_response() {
        use crate::sync::hash_comparison::{LeafMetadata, TreeLeafData, TreeNode};

        let leaf_data = TreeLeafData::new(
            [10u8; 32],
            vec![1, 2, 3],
            LeafMetadata::new(
                crate::sync::hash_comparison::CrdtType::LwwRegister,
                100,
                [0u8; 32],
            ),
        );
        let node = TreeNode::leaf([1u8; 32], [2u8; 32], leaf_data);

        let response = MessagePayload::TreeNodeResponse {
            nodes: vec![node],
            not_found: false,
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::TreeNodeResponse { nodes, not_found } => {
                assert_eq!(nodes.len(), 1);
                assert!(!not_found);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_message_payload_tree_node_response_not_found() {
        let response = MessagePayload::TreeNodeResponse {
            nodes: vec![],
            not_found: true,
        };

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: MessagePayload = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            MessagePayload::TreeNodeResponse { nodes, not_found } => {
                assert!(nodes.is_empty());
                assert!(not_found);
            }
            _ => panic!("wrong variant"),
        }
    }
}
