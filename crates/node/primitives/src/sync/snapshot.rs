//! Snapshot sync types and wire protocol messages (CIP §6 - Snapshot Sync Constraints).
//!
//! Types for snapshot-based synchronization and general sync wire messages.
//!
//! # When to Use
//!
//! - **ONLY** for fresh nodes with NO existing state (Invariant I5)
//! - When delta history is pruned and state-based sync is impossible
//! - For initial bootstrap of new nodes joining a context
//!
//! # Critical Invariants
//!
//! - **I5**: Initialized nodes MUST use CRDT merge, NEVER snapshot overwrite
//! - **I7**: Root hash MUST be verified BEFORE applying any snapshot data
//!
//! # Validation
//!
//! All types have `is_valid()` methods that should be called after deserializing
//! from untrusted sources to prevent resource exhaustion attacks.

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_crypto::Nonce;
use calimero_network_primitives::specialized_node_invite::SpecializedNodeType;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey};

use super::hash_comparison::LeafMetadata;

// =============================================================================
// Constants
// =============================================================================

/// Default page size for snapshot transfer (256 KB).
///
/// Balances between memory usage and transfer efficiency.
pub const DEFAULT_SNAPSHOT_PAGE_SIZE: u32 = 256 * 1024;

/// Maximum page size for snapshot transfer (4 MB).
///
/// Limits memory usage for individual pages to prevent DoS attacks.
pub const MAX_SNAPSHOT_PAGE_SIZE: u32 = 4 * 1024 * 1024;

/// Maximum entities per snapshot page.
///
/// Limits the size of `SnapshotEntityPage::entities` to prevent
/// memory exhaustion from malicious peers.
pub const MAX_ENTITIES_PER_PAGE: usize = 1_000;

/// Maximum total pages in a snapshot transfer.
///
/// Prevents unbounded memory allocation during snapshot reception.
/// At 256KB per page, this allows ~2.5GB total transfer.
pub const MAX_SNAPSHOT_PAGES: usize = 10_000;

/// Maximum entity data size (1 MB).
///
/// Limits individual entity payload to prevent memory exhaustion.
pub const MAX_ENTITY_DATA_SIZE: usize = 1_048_576;

/// Maximum DAG heads in a snapshot completion message.
///
/// Limits the size of `SnapshotComplete::dag_heads`.
pub const MAX_DAG_HEADS: usize = 100;

/// Maximum compressed payload size (8 MB).
///
/// Limits the size of compressed snapshot page payloads to prevent
/// memory exhaustion before decompression. Set higher than uncompressed
/// limit to allow for edge cases where compression expands data.
pub const MAX_COMPRESSED_PAYLOAD_SIZE: usize = 8 * 1024 * 1024;

// =============================================================================
// Snapshot Boundary Types
// =============================================================================

/// Request to negotiate a snapshot boundary for sync.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotBoundaryRequest {
    /// Context being synchronized.
    pub context_id: ContextId,

    /// Optional hint for boundary timestamp (nanoseconds since epoch).
    pub requested_cutoff_timestamp: Option<u64>,
}

/// Response to snapshot boundary negotiation.
///
/// Contains the authoritative boundary state that the responder will serve
/// for the duration of this sync session.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotBoundaryResponse {
    /// Authoritative boundary timestamp (nanoseconds since epoch).
    pub boundary_timestamp: u64,

    /// Root hash for the boundary state; must be verified after apply.
    pub boundary_root_hash: Hash,

    /// Peer's DAG heads at the boundary; used for fine-sync after snapshot.
    pub dag_heads: Vec<[u8; 32]>,
}

impl SnapshotBoundaryResponse {
    /// Check if response is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.dag_heads.len() <= MAX_DAG_HEADS
    }
}

/// Request to stream snapshot pages.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotStreamRequest {
    /// Context being synchronized.
    pub context_id: ContextId,

    /// Boundary root hash from the negotiated boundary.
    pub boundary_root_hash: Hash,

    /// Maximum number of pages to send in a burst.
    pub page_limit: u16,

    /// Maximum uncompressed bytes per page.
    pub byte_limit: u32,

    /// Optional cursor to resume paging.
    pub resume_cursor: Option<Vec<u8>>,
}

impl SnapshotStreamRequest {
    /// Get the validated byte limit.
    ///
    /// Clamps to MAX_SNAPSHOT_PAGE_SIZE to prevent memory exhaustion.
    #[must_use]
    pub fn validated_byte_limit(&self) -> u32 {
        if self.byte_limit == 0 {
            DEFAULT_SNAPSHOT_PAGE_SIZE
        } else {
            self.byte_limit.min(MAX_SNAPSHOT_PAGE_SIZE)
        }
    }
}

/// A page of snapshot data (raw bytes format).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotPage {
    /// Compressed payload (lz4).
    pub payload: Vec<u8>,
    /// Expected size after decompression.
    pub uncompressed_len: u32,
    /// Next cursor; `None` indicates completion.
    pub cursor: Option<Vec<u8>>,
    /// Total pages in this stream (estimate).
    pub page_count: u64,
    /// Pages sent so far.
    pub sent_count: u64,
}

impl SnapshotPage {
    /// Check if this is the last page.
    #[must_use]
    pub fn is_last(&self) -> bool {
        self.cursor.is_none()
    }

    /// Check if page is within valid bounds.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.uncompressed_len <= MAX_SNAPSHOT_PAGE_SIZE
            && self.page_count <= MAX_SNAPSHOT_PAGES as u64
            && self.sent_count <= self.page_count
            && self.payload.len() <= MAX_COMPRESSED_PAYLOAD_SIZE
    }
}

/// Cursor for resuming snapshot pagination.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotCursor {
    /// Last key sent in canonical order.
    pub last_key: [u8; 32],
}

// =============================================================================
// Snapshot Bootstrap Types (CIP §6 - Snapshot Sync Constraints)
// =============================================================================

/// Request to initiate a full snapshot transfer.
///
/// CRITICAL: This is ONLY for fresh nodes with NO existing state.
/// Invariant I5: Initialized nodes MUST use CRDT merge, not snapshot overwrite.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotRequest {
    /// Whether to compress the snapshot data.
    pub compressed: bool,

    /// Maximum page size in bytes (0 = use responder's default).
    pub max_page_size: u32,

    /// Whether the initiator is definitely a fresh node (for safety check).
    /// If false, responder SHOULD verify this claim.
    pub is_fresh_node: bool,
}

impl SnapshotRequest {
    /// Create a request for compressed snapshot.
    #[must_use]
    pub fn compressed() -> Self {
        Self {
            compressed: true,
            max_page_size: 0,
            is_fresh_node: true,
        }
    }

    /// Create a request for uncompressed snapshot.
    #[must_use]
    pub fn uncompressed() -> Self {
        Self {
            compressed: false,
            max_page_size: 0,
            is_fresh_node: true,
        }
    }

    /// Set maximum page size.
    #[must_use]
    pub fn with_max_page_size(mut self, size: u32) -> Self {
        self.max_page_size = size;
        self
    }

    /// Get the validated page size.
    ///
    /// Returns DEFAULT_SNAPSHOT_PAGE_SIZE if 0, otherwise clamps to MAX.
    #[must_use]
    pub fn validated_page_size(&self) -> u32 {
        if self.max_page_size == 0 {
            DEFAULT_SNAPSHOT_PAGE_SIZE
        } else {
            self.max_page_size.min(MAX_SNAPSHOT_PAGE_SIZE)
        }
    }
}

/// A single entity in a snapshot.
///
/// Contains all information needed to reconstruct the entity.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotEntity {
    /// Entity ID (deterministic, based on path).
    pub id: [u8; 32],

    /// Serialized entity data.
    pub data: Vec<u8>,

    /// Entity metadata (crdt_type, timestamps, etc.).
    pub metadata: LeafMetadata,

    /// Collection ID this entity belongs to.
    pub collection_id: [u8; 32],

    /// Parent entity ID (for nested structures).
    pub parent_id: Option<[u8; 32]>,
}

impl SnapshotEntity {
    /// Create a new snapshot entity.
    #[must_use]
    pub fn new(
        id: [u8; 32],
        data: Vec<u8>,
        metadata: LeafMetadata,
        collection_id: [u8; 32],
    ) -> Self {
        Self {
            id,
            data,
            metadata,
            collection_id,
            parent_id: None,
        }
    }

    /// Set parent entity ID.
    #[must_use]
    pub fn with_parent(mut self, parent_id: [u8; 32]) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    /// Check if this is a root-level entity.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.parent_id.is_none()
    }

    /// Check if entity is within valid bounds.
    ///
    /// Validates data size to prevent memory exhaustion from malicious peers.
    /// LeafMetadata has fixed-size fields, so it's always valid if present.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.data.len() <= MAX_ENTITY_DATA_SIZE
    }
}

/// A page of snapshot entities for paginated transfer.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotEntityPage {
    /// Page number (0-indexed).
    pub page_number: usize,

    /// Total number of pages (may be estimated).
    pub total_pages: usize,

    /// Entities in this page.
    pub entities: Vec<SnapshotEntity>,

    /// Whether this is the last page.
    pub is_last: bool,
}

impl SnapshotEntityPage {
    /// Create a new snapshot page.
    #[must_use]
    pub fn new(
        page_number: usize,
        total_pages: usize,
        entities: Vec<SnapshotEntity>,
        is_last: bool,
    ) -> Self {
        Self {
            page_number,
            total_pages,
            entities,
            is_last,
        }
    }

    /// Number of entities in this page.
    #[must_use]
    pub fn entity_count(&self) -> usize {
        self.entities.len()
    }

    /// Check if this page is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty()
    }

    /// Check if page is within valid bounds.
    ///
    /// Call this after deserializing from untrusted sources.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        // Check entity count limit
        if self.entities.len() > MAX_ENTITIES_PER_PAGE {
            return false;
        }

        // Check total pages limit
        if self.total_pages > MAX_SNAPSHOT_PAGES {
            return false;
        }

        // Check page number is within bounds (page_number is 0-indexed)
        if self.total_pages > 0 && self.page_number >= self.total_pages {
            return false;
        }

        // Check is_last coherence: if is_last, must be the final page
        if self.is_last && self.total_pages > 0 && self.page_number + 1 != self.total_pages {
            return false;
        }

        // Validate all entities
        self.entities.iter().all(SnapshotEntity::is_valid)
    }
}

/// Completion marker for snapshot transfer.
///
/// Sent after all pages have been transferred.
/// Contains verification information for Invariant I7.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotComplete {
    /// Root hash of the complete snapshot.
    /// INVARIANT I7: MUST be verified before applying any entities.
    pub root_hash: [u8; 32],

    /// Total number of entities transferred.
    pub total_entities: usize,

    /// Total number of pages transferred.
    pub total_pages: usize,

    /// Uncompressed size in bytes.
    pub uncompressed_size: u64,

    /// Compressed size in bytes (if compression was used).
    pub compressed_size: Option<u64>,

    /// DAG heads at the time of snapshot.
    /// Used to create checkpoint delta after apply.
    pub dag_heads: Vec<[u8; 32]>,
}

impl SnapshotComplete {
    /// Create a new snapshot completion marker.
    #[must_use]
    pub fn new(
        root_hash: [u8; 32],
        total_entities: usize,
        total_pages: usize,
        uncompressed_size: u64,
    ) -> Self {
        Self {
            root_hash,
            total_entities,
            total_pages,
            uncompressed_size,
            compressed_size: None,
            dag_heads: vec![],
        }
    }

    /// Set compressed size.
    #[must_use]
    pub fn with_compressed_size(mut self, size: u64) -> Self {
        self.compressed_size = Some(size);
        self
    }

    /// Set DAG heads.
    #[must_use]
    pub fn with_dag_heads(mut self, heads: Vec<[u8; 32]>) -> Self {
        self.dag_heads = heads;
        self
    }

    /// Calculate compression ratio (if compression was used).
    #[must_use]
    pub fn compression_ratio(&self) -> Option<f64> {
        self.compressed_size
            .map(|c| c as f64 / self.uncompressed_size.max(1) as f64)
    }

    /// Check if completion is within valid bounds.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.total_pages <= MAX_SNAPSHOT_PAGES && self.dag_heads.len() <= MAX_DAG_HEADS
    }
}

// =============================================================================
// Snapshot Verification (Invariant I7)
// =============================================================================

/// Result of verifying a snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapshotVerifyResult {
    /// Verification passed - safe to apply.
    Valid,

    /// Root hash mismatch - DO NOT apply.
    RootHashMismatch {
        expected: [u8; 32],
        computed: [u8; 32],
    },

    /// Entity count mismatch.
    EntityCountMismatch { expected: usize, actual: usize },

    /// Missing pages detected.
    MissingPages { missing: Vec<usize> },
}

impl SnapshotVerifyResult {
    /// Check if verification passed.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// Convert to error if invalid.
    #[must_use]
    pub fn to_error(&self) -> Option<SnapshotError> {
        match self {
            Self::Valid => None,
            Self::RootHashMismatch { expected, computed } => {
                Some(SnapshotError::RootHashMismatch {
                    expected: *expected,
                    computed: *computed,
                })
            }
            Self::EntityCountMismatch { expected, actual } => {
                Some(SnapshotError::EntityCountMismatch {
                    expected: *expected,
                    actual: *actual,
                })
            }
            Self::MissingPages { missing } => Some(SnapshotError::MissingPages {
                missing: missing.clone(),
            }),
        }
    }
}

// =============================================================================
// Snapshot Errors
// =============================================================================

/// Errors that can occur during snapshot sync.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum SnapshotError {
    /// Peer's delta history is pruned; full snapshot required.
    SnapshotRequired,

    /// The requested boundary is invalid or no longer available.
    InvalidBoundary,

    /// Resume cursor is invalid or expired.
    ResumeCursorInvalid,

    /// Attempted to apply snapshot on a node with existing state.
    /// INVARIANT I5: Snapshot is ONLY for fresh nodes.
    SnapshotOnInitializedNode,

    /// Root hash verification failed.
    /// INVARIANT I7: Verification REQUIRED before apply.
    RootHashMismatch {
        expected: [u8; 32],
        computed: [u8; 32],
    },

    /// Snapshot transfer was interrupted.
    TransferInterrupted { pages_received: usize },

    /// Decompression failed.
    DecompressionFailed,

    /// Entity count does not match expected count.
    EntityCountMismatch { expected: usize, actual: usize },

    /// Some pages are missing from the snapshot transfer.
    MissingPages { missing: Vec<usize> },
}

// =============================================================================
// Safety Functions
// =============================================================================

/// Safety check before applying snapshot.
///
/// Returns error if the local node has existing state.
/// INVARIANT I5: Snapshot is ONLY for fresh nodes.
pub fn check_snapshot_safety(has_local_state: bool) -> Result<(), SnapshotError> {
    if has_local_state {
        Err(SnapshotError::SnapshotOnInitializedNode)
    } else {
        Ok(())
    }
}

// =============================================================================
// Wire Protocol Messages
// =============================================================================

#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "Of no consequence here")]
pub enum BroadcastMessage<'a> {
    StateDelta {
        context_id: ContextId,
        author_id: PublicKey,

        /// DAG: Unique delta ID (content hash)
        delta_id: [u8; 32],

        /// DAG: Parent delta IDs (for causal ordering)
        parent_ids: Vec<[u8; 32]>,

        /// Hybrid Logical Clock timestamp for causal ordering
        hlc: calimero_storage::logical_clock::HybridTimestamp,

        root_hash: Hash, // todo! shouldn't be cleartext
        artifact: Cow<'a, [u8]>,
        nonce: Nonce,

        /// Execution events that were emitted during the state change.
        /// This field is encrypted along with the artifact.
        events: Option<Cow<'a, [u8]>>,
    },

    /// Hash heartbeat for divergence detection
    ///
    /// Periodically broadcast by nodes to allow peers to detect silent divergence.
    /// If a peer has a different hash for the same DAG heads, it indicates a problem.
    HashHeartbeat {
        context_id: ContextId,
        /// Current root hash
        root_hash: Hash,
        /// Current DAG head(s)
        dag_heads: Vec<[u8; 32]>,
    },

    /// Specialized node discovery request
    ///
    /// Broadcast by a node to discover and invite specialized nodes (e.g., read-only TEE nodes).
    /// Specialized nodes receiving this will respond via request-response protocol
    /// to the message source (available from gossipsub message).
    ///
    /// Note: context_id is NOT included - it's tracked internally by the requesting
    /// node using the nonce as the lookup key.
    SpecializedNodeDiscovery {
        /// Random nonce to bind verification to this request
        nonce: [u8; 32],
        /// Type of specialized node being invited
        node_type: SpecializedNodeType,
    },

    /// Confirmation that a specialized node has joined a context
    ///
    /// Broadcast by specialized nodes on the context topic after successfully joining.
    /// The inviting node receives this and removes the pending invite entry.
    SpecializedNodeJoinConfirmation {
        /// The nonce from the original discovery request
        nonce: [u8; 32],
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum StreamMessage<'a> {
    Init {
        context_id: ContextId,
        party_id: PublicKey,
        payload: InitPayload,
        next_nonce: Nonce,
    },
    Message {
        sequence_id: usize,
        payload: MessagePayload<'a>,
        next_nonce: Nonce,
    },
    /// Other peers must not learn anything about the node's state if anything goes wrong.
    OpaqueError,
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum InitPayload {
    BlobShare {
        blob_id: BlobId,
    },
    KeyShare,
    /// Request a specific delta by ID (for DAG gap filling)
    DeltaRequest {
        context_id: ContextId,
        delta_id: [u8; 32],
    },
    /// Request peer's current DAG heads for catchup
    DagHeadsRequest {
        context_id: ContextId,
    },
    /// Request snapshot boundary negotiation.
    SnapshotBoundaryRequest {
        context_id: ContextId,
        requested_cutoff_timestamp: Option<u64>,
    },
    /// Request to stream snapshot pages.
    SnapshotStreamRequest {
        context_id: ContextId,
        boundary_root_hash: Hash,
        page_limit: u16,
        byte_limit: u32,
        resume_cursor: Option<Vec<u8>>,
    },
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub enum MessagePayload<'a> {
    BlobShare {
        chunk: Cow<'a, [u8]>,
    },
    KeyShare {
        sender_key: PrivateKey,
    },
    /// Response to DeltaRequest containing the requested delta
    DeltaResponse {
        delta: Cow<'a, [u8]>,
    },
    /// Delta not found response
    DeltaNotFound,
    /// Response to DagHeadsRequest containing peer's current heads and root hash
    DagHeadsResponse {
        dag_heads: Vec<[u8; 32]>,
        root_hash: Hash,
    },
    /// Challenge to prove ownership of claimed identity
    Challenge {
        challenge: [u8; 32],
    },
    /// Response to challenge with signature (Ed25519 signature is 64 bytes)
    ChallengeResponse {
        signature: [u8; 64],
    },
    /// Response to SnapshotBoundaryRequest
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
        payload: Cow<'a, [u8]>,
        uncompressed_len: u32,
        cursor: Option<Vec<u8>>,
        page_count: u64,
        sent_count: u64,
    },
    /// Snapshot sync error.
    SnapshotError {
        error: SnapshotError,
    },
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::hash_comparison::CrdtType;

    // =========================================================================
    // Helper Functions
    // =========================================================================

    fn make_metadata() -> LeafMetadata {
        LeafMetadata::new(CrdtType::LwwRegister, 100, [1; 32])
    }

    fn make_entity(id: u8, data: Vec<u8>) -> SnapshotEntity {
        SnapshotEntity::new([id; 32], data, make_metadata(), [2; 32])
    }

    // =========================================================================
    // SnapshotRequest Tests
    // =========================================================================

    #[test]
    fn test_snapshot_request_compressed() {
        let request = SnapshotRequest::compressed();

        assert!(request.compressed);
        assert!(request.is_fresh_node);
        assert_eq!(request.max_page_size, 0);
        assert_eq!(request.validated_page_size(), DEFAULT_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_request_uncompressed() {
        let request = SnapshotRequest::uncompressed().with_max_page_size(1024 * 1024);

        assert!(!request.compressed);
        assert_eq!(request.max_page_size, 1024 * 1024);
        assert_eq!(request.validated_page_size(), 1024 * 1024);
    }

    #[test]
    fn test_snapshot_request_page_size_clamping() {
        let request = SnapshotRequest::compressed().with_max_page_size(u32::MAX);

        // Should clamp to MAX_SNAPSHOT_PAGE_SIZE
        assert_eq!(request.validated_page_size(), MAX_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_request_roundtrip() {
        let request = SnapshotRequest::compressed().with_max_page_size(65536);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: SnapshotRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
    }

    // =========================================================================
    // SnapshotEntity Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_new() {
        let entity = make_entity(1, vec![1, 2, 3]);

        assert_eq!(entity.id, [1; 32]);
        assert!(entity.is_root());
        assert!(entity.parent_id.is_none());
        assert!(entity.is_valid());
    }

    #[test]
    fn test_snapshot_entity_with_parent() {
        let entity = make_entity(2, vec![4, 5, 6]).with_parent([1; 32]);

        assert!(!entity.is_root());
        assert_eq!(entity.parent_id, Some([1; 32]));
        assert!(entity.is_valid());
    }

    #[test]
    fn test_snapshot_entity_validation() {
        // Valid entity
        let valid = make_entity(1, vec![1, 2, 3]);
        assert!(valid.is_valid());

        // Invalid entity: oversized data
        let oversized = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        assert!(!oversized.is_valid());
    }

    #[test]
    fn test_snapshot_entity_roundtrip() {
        let entity = make_entity(3, vec![7, 8, 9]).with_parent([2; 32]);

        let encoded = borsh::to_vec(&entity).expect("serialize");
        let decoded: SnapshotEntity = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(entity, decoded);
    }

    // =========================================================================
    // SnapshotEntityPage Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_page() {
        let entity1 = make_entity(1, vec![1, 2]);
        let entity2 = make_entity(2, vec![3, 4]);

        let page = SnapshotEntityPage::new(0, 3, vec![entity1, entity2], false);

        assert_eq!(page.page_number, 0);
        assert_eq!(page.total_pages, 3);
        assert_eq!(page.entity_count(), 2);
        assert!(!page.is_last);
        assert!(!page.is_empty());
        assert!(page.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_last() {
        let entity = make_entity(1, vec![1, 2, 3]);
        let page = SnapshotEntityPage::new(2, 3, vec![entity], true);

        assert!(page.is_last);
        assert!(page.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_empty() {
        let page = SnapshotEntityPage::new(0, 1, vec![], true);

        assert!(page.is_empty());
        assert_eq!(page.entity_count(), 0);
        assert!(page.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_validation() {
        // Valid page at entity limit
        let entities: Vec<SnapshotEntity> = (0..MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity(i as u8, vec![i as u8]))
            .collect();
        let at_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(at_limit.is_valid());

        // Invalid page: over entity limit
        let entities: Vec<SnapshotEntity> = (0..=MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity(i as u8, vec![i as u8]))
            .collect();
        let over_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(!over_limit.is_valid());

        // Invalid page: over total pages limit
        let entity = make_entity(1, vec![1]);
        let over_pages = SnapshotEntityPage::new(0, MAX_SNAPSHOT_PAGES + 1, vec![entity], false);
        assert!(!over_pages.is_valid());

        // Invalid page: page_number >= total_pages
        let entity = make_entity(1, vec![1]);
        let invalid_page_num = SnapshotEntityPage::new(5, 3, vec![entity], false);
        assert!(!invalid_page_num.is_valid());

        // Invalid page: is_last but not the final page
        let entity = make_entity(1, vec![1]);
        let invalid_last = SnapshotEntityPage::new(0, 3, vec![entity], true);
        assert!(!invalid_last.is_valid());

        // Valid page: is_last and is the final page
        let entity = make_entity(1, vec![1]);
        let valid_last = SnapshotEntityPage::new(2, 3, vec![entity], true);
        assert!(valid_last.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_roundtrip() {
        let entity = make_entity(4, vec![10, 11]);
        let page = SnapshotEntityPage::new(1, 5, vec![entity], false);

        let encoded = borsh::to_vec(&page).expect("serialize");
        let decoded: SnapshotEntityPage = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(page, decoded);
    }

    // =========================================================================
    // SnapshotComplete Tests
    // =========================================================================

    #[test]
    fn test_snapshot_complete() {
        let complete = SnapshotComplete::new([1; 32], 1000, 10, 1024 * 1024)
            .with_compressed_size(256 * 1024)
            .with_dag_heads(vec![[2; 32], [3; 32]]);

        assert_eq!(complete.root_hash, [1; 32]);
        assert_eq!(complete.total_entities, 1000);
        assert_eq!(complete.total_pages, 10);
        assert_eq!(complete.dag_heads.len(), 2);
        assert!(complete.is_valid());

        // Compression ratio: 256KB / 1MB = 0.25
        let ratio = complete.compression_ratio().unwrap();
        assert!((ratio - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_snapshot_complete_no_compression() {
        let complete = SnapshotComplete::new([1; 32], 100, 1, 10000);

        assert!(complete.compression_ratio().is_none());
        assert!(complete.is_valid());
    }

    #[test]
    fn test_snapshot_complete_validation() {
        // Valid completion
        let valid = SnapshotComplete::new([1; 32], 1000, 10, 1024 * 1024);
        assert!(valid.is_valid());

        // Invalid: too many pages
        let over_pages = SnapshotComplete::new([1; 32], 1000, MAX_SNAPSHOT_PAGES + 1, 1024);
        assert!(!over_pages.is_valid());

        // Invalid: too many DAG heads
        let heads: Vec<[u8; 32]> = (0..=MAX_DAG_HEADS).map(|i| [i as u8; 32]).collect();
        let over_heads = SnapshotComplete::new([1; 32], 1000, 10, 1024).with_dag_heads(heads);
        assert!(!over_heads.is_valid());
    }

    #[test]
    fn test_snapshot_complete_roundtrip() {
        let complete = SnapshotComplete::new([1; 32], 500, 5, 512 * 1024)
            .with_compressed_size(128 * 1024)
            .with_dag_heads(vec![[2; 32]]);

        let encoded = borsh::to_vec(&complete).expect("serialize");
        let decoded: SnapshotComplete = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(complete, decoded);
    }

    // =========================================================================
    // SnapshotVerifyResult Tests
    // =========================================================================

    #[test]
    fn test_snapshot_verify_result_valid() {
        let result = SnapshotVerifyResult::Valid;
        assert!(result.is_valid());
        assert!(result.to_error().is_none());
    }

    #[test]
    fn test_snapshot_verify_result_hash_mismatch() {
        let result = SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32],
        };
        assert!(!result.is_valid());

        let error = result.to_error().unwrap();
        assert!(matches!(error, SnapshotError::RootHashMismatch { .. }));
    }

    #[test]
    fn test_snapshot_verify_result_entity_count() {
        let result = SnapshotVerifyResult::EntityCountMismatch {
            expected: 100,
            actual: 99,
        };
        assert!(!result.is_valid());
        let error = result.to_error().unwrap();
        assert!(matches!(
            error,
            SnapshotError::EntityCountMismatch {
                expected: 100,
                actual: 99
            }
        ));
    }

    #[test]
    fn test_snapshot_verify_result_missing_pages() {
        let result = SnapshotVerifyResult::MissingPages {
            missing: vec![3, 5, 7],
        };
        assert!(!result.is_valid());
        let error = result.to_error().unwrap();
        match error {
            SnapshotError::MissingPages { missing } => {
                assert_eq!(missing, vec![3, 5, 7]);
            }
            _ => panic!("Expected MissingPages error"),
        }
    }

    // =========================================================================
    // Safety Function Tests (Invariant I5)
    // =========================================================================

    #[test]
    fn test_check_snapshot_safety_fresh_node() {
        // Fresh node (no state) - OK
        assert!(check_snapshot_safety(false).is_ok());
    }

    #[test]
    fn test_check_snapshot_safety_initialized_node() {
        // Initialized node (has state) - ERROR
        let result = check_snapshot_safety(true);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SnapshotError::SnapshotOnInitializedNode
        ));
    }

    // =========================================================================
    // SnapshotError Tests
    // =========================================================================

    #[test]
    fn test_snapshot_error_roundtrip() {
        let errors = vec![
            SnapshotError::SnapshotRequired,
            SnapshotError::InvalidBoundary,
            SnapshotError::ResumeCursorInvalid,
            SnapshotError::SnapshotOnInitializedNode,
            SnapshotError::RootHashMismatch {
                expected: [1; 32],
                computed: [2; 32],
            },
            SnapshotError::TransferInterrupted { pages_received: 5 },
            SnapshotError::DecompressionFailed,
            SnapshotError::EntityCountMismatch {
                expected: 100,
                actual: 99,
            },
            SnapshotError::MissingPages {
                missing: vec![3, 5, 7],
            },
        ];

        for error in errors {
            let encoded = borsh::to_vec(&error).expect("serialize");
            let decoded: SnapshotError = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(error, decoded);
        }
    }

    // =========================================================================
    // SnapshotBoundaryResponse Tests
    // =========================================================================

    #[test]
    fn test_snapshot_boundary_response_validation() {
        // Valid response
        let valid = SnapshotBoundaryResponse {
            boundary_timestamp: 12345,
            boundary_root_hash: Hash::default(),
            dag_heads: vec![[1; 32], [2; 32]],
        };
        assert!(valid.is_valid());

        // Invalid: too many DAG heads
        let heads: Vec<[u8; 32]> = (0..=MAX_DAG_HEADS).map(|i| [i as u8; 32]).collect();
        let invalid = SnapshotBoundaryResponse {
            boundary_timestamp: 12345,
            boundary_root_hash: Hash::default(),
            dag_heads: heads,
        };
        assert!(!invalid.is_valid());
    }

    // =========================================================================
    // SnapshotStreamRequest Tests
    // =========================================================================

    #[test]
    fn test_snapshot_stream_request_byte_limit() {
        // Zero returns default
        let request = SnapshotStreamRequest {
            context_id: ContextId::zero(),
            boundary_root_hash: Hash::default(),
            page_limit: 10,
            byte_limit: 0,
            resume_cursor: None,
        };
        assert_eq!(request.validated_byte_limit(), DEFAULT_SNAPSHOT_PAGE_SIZE);

        // Normal value passes through
        let request2 = SnapshotStreamRequest {
            context_id: ContextId::zero(),
            boundary_root_hash: Hash::default(),
            page_limit: 10,
            byte_limit: 100_000,
            resume_cursor: None,
        };
        assert_eq!(request2.validated_byte_limit(), 100_000);

        // Excessive value is clamped
        let request3 = SnapshotStreamRequest {
            context_id: ContextId::zero(),
            boundary_root_hash: Hash::default(),
            page_limit: 10,
            byte_limit: u32::MAX,
            resume_cursor: None,
        };
        assert_eq!(request3.validated_byte_limit(), MAX_SNAPSHOT_PAGE_SIZE);
    }

    // =========================================================================
    // SnapshotPage Tests
    // =========================================================================

    #[test]
    fn test_snapshot_page_is_last() {
        let page_not_last = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: 100,
            cursor: Some(vec![4, 5]),
            page_count: 10,
            sent_count: 5,
        };
        assert!(!page_not_last.is_last());

        let page_is_last = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: 100,
            cursor: None,
            page_count: 10,
            sent_count: 10,
        };
        assert!(page_is_last.is_last());
    }

    #[test]
    fn test_snapshot_page_validation() {
        // Valid page
        let valid = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: 100,
            cursor: None,
            page_count: 10,
            sent_count: 10,
        };
        assert!(valid.is_valid());

        // Invalid: oversized uncompressed_len
        let oversized = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: MAX_SNAPSHOT_PAGE_SIZE + 1,
            cursor: None,
            page_count: 10,
            sent_count: 10,
        };
        assert!(!oversized.is_valid());

        // Invalid: too many pages
        let too_many = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: 100,
            cursor: None,
            page_count: MAX_SNAPSHOT_PAGES as u64 + 1,
            sent_count: 10,
        };
        assert!(!too_many.is_valid());

        // Invalid: sent_count > page_count
        let invalid_sent = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: 100,
            cursor: None,
            page_count: 5,
            sent_count: 10,
        };
        assert!(!invalid_sent.is_valid());

        // Invalid: oversized compressed payload
        let oversized_payload = SnapshotPage {
            payload: vec![0u8; MAX_COMPRESSED_PAYLOAD_SIZE + 1],
            uncompressed_len: 100,
            cursor: None,
            page_count: 10,
            sent_count: 10,
        };
        assert!(!oversized_payload.is_valid());
    }

    // =========================================================================
    // Boundary Condition Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_data_at_limit() {
        // Exactly at MAX_ENTITY_DATA_SIZE - should be valid
        let at_limit = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE]);
        assert!(at_limit.is_valid());

        // One byte over - should be invalid
        let over_limit = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_entity_page_at_entity_limit() {
        // Exactly at MAX_ENTITIES_PER_PAGE - should be valid
        let entities: Vec<SnapshotEntity> = (0..MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity((i % 256) as u8, vec![(i % 256) as u8]))
            .collect();
        let at_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(at_limit.is_valid());
        assert_eq!(at_limit.entity_count(), MAX_ENTITIES_PER_PAGE);

        // One entity over - should be invalid
        let entities: Vec<SnapshotEntity> = (0..=MAX_ENTITIES_PER_PAGE)
            .map(|i| make_entity((i % 256) as u8, vec![(i % 256) as u8]))
            .collect();
        let over_limit = SnapshotEntityPage::new(0, 1, entities, true);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_complete_at_page_limit() {
        // Exactly at MAX_SNAPSHOT_PAGES - should be valid
        let at_limit = SnapshotComplete::new([1; 32], 1000, MAX_SNAPSHOT_PAGES, 1024);
        assert!(at_limit.is_valid());

        // One page over - should be invalid
        let over_limit = SnapshotComplete::new([1; 32], 1000, MAX_SNAPSHOT_PAGES + 1, 1024);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_complete_at_dag_heads_limit() {
        // Exactly at MAX_DAG_HEADS - should be valid
        let heads: Vec<[u8; 32]> = (0..MAX_DAG_HEADS).map(|i| [(i % 256) as u8; 32]).collect();
        let at_limit = SnapshotComplete::new([1; 32], 1000, 10, 1024).with_dag_heads(heads);
        assert!(at_limit.is_valid());

        // One head over - should be invalid
        let heads: Vec<[u8; 32]> = (0..=MAX_DAG_HEADS).map(|i| [(i % 256) as u8; 32]).collect();
        let over_limit = SnapshotComplete::new([1; 32], 1000, 10, 1024).with_dag_heads(heads);
        assert!(!over_limit.is_valid());
    }

    #[test]
    fn test_snapshot_page_at_size_limit() {
        // Exactly at MAX_SNAPSHOT_PAGE_SIZE - should be valid
        let at_limit = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: MAX_SNAPSHOT_PAGE_SIZE,
            cursor: None,
            page_count: 10,
            sent_count: 10,
        };
        assert!(at_limit.is_valid());

        // One byte over - should be invalid
        let over_limit = SnapshotPage {
            payload: vec![1, 2, 3],
            uncompressed_len: MAX_SNAPSHOT_PAGE_SIZE + 1,
            cursor: None,
            page_count: 10,
            sent_count: 10,
        };
        assert!(!over_limit.is_valid());
    }

    // =========================================================================
    // Security / Exploit Prevention Tests
    // =========================================================================

    #[test]
    fn test_snapshot_request_memory_exhaustion_prevention() {
        // Attempt to request extremely large page size - should be clamped
        let request = SnapshotRequest::compressed().with_max_page_size(u32::MAX);
        assert_eq!(request.validated_page_size(), MAX_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_stream_request_memory_exhaustion_prevention() {
        // Attempt extremely large byte limit - should be clamped
        let request = SnapshotStreamRequest {
            context_id: ContextId::zero(),
            boundary_root_hash: Hash::default(),
            page_limit: u16::MAX,
            byte_limit: u32::MAX,
            resume_cursor: None,
        };
        assert_eq!(request.validated_byte_limit(), MAX_SNAPSHOT_PAGE_SIZE);
    }

    #[test]
    fn test_snapshot_entity_page_cross_validation() {
        // Page containing an invalid entity should be invalid
        let invalid_entity = make_entity(1, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        let page = SnapshotEntityPage::new(0, 1, vec![invalid_entity], true);
        assert!(!page.is_valid());

        // Page with mix of valid and invalid entities
        let valid_entity = make_entity(1, vec![1, 2, 3]);
        let invalid_entity = make_entity(2, vec![0u8; MAX_ENTITY_DATA_SIZE + 1]);
        let mixed_page = SnapshotEntityPage::new(0, 1, vec![valid_entity, invalid_entity], true);
        assert!(!mixed_page.is_valid());
    }

    #[test]
    fn test_snapshot_complete_compression_ratio_zero_uncompressed() {
        // Edge case: zero uncompressed size (uses max(1) to prevent division by zero)
        let complete = SnapshotComplete::new([1; 32], 0, 0, 0).with_compressed_size(100);

        let ratio = complete.compression_ratio().unwrap();
        // 100 / max(0, 1) = 100.0
        assert_eq!(ratio, 100.0);
    }

    // =========================================================================
    // Special Values Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_all_zeros() {
        let entity = SnapshotEntity::new([0u8; 32], vec![], make_metadata(), [0u8; 32]);
        assert!(entity.is_valid());
        assert!(entity.is_root());
        assert!(entity.data.is_empty());

        // Roundtrip
        let encoded = borsh::to_vec(&entity).expect("serialize");
        let decoded: SnapshotEntity = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(entity, decoded);
    }

    #[test]
    fn test_snapshot_entity_all_ones() {
        let entity = SnapshotEntity::new([0xFF; 32], vec![0xFF; 100], make_metadata(), [0xFF; 32])
            .with_parent([0xFF; 32]);
        assert!(entity.is_valid());
        assert!(!entity.is_root());

        // Roundtrip
        let encoded = borsh::to_vec(&entity).expect("serialize");
        let decoded: SnapshotEntity = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(entity, decoded);
    }

    #[test]
    fn test_snapshot_complete_all_zeros() {
        let complete = SnapshotComplete::new([0u8; 32], 0, 0, 0);
        assert!(complete.is_valid());
        assert!(complete.compression_ratio().is_none());
        assert!(complete.dag_heads.is_empty());

        // Roundtrip
        let encoded = borsh::to_vec(&complete).expect("serialize");
        let decoded: SnapshotComplete = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(complete, decoded);
    }

    #[test]
    fn test_snapshot_complete_max_values() {
        let complete = SnapshotComplete::new([0xFF; 32], usize::MAX, MAX_SNAPSHOT_PAGES, u64::MAX)
            .with_compressed_size(u64::MAX);
        assert!(complete.is_valid());

        // Roundtrip
        let encoded = borsh::to_vec(&complete).expect("serialize");
        let decoded: SnapshotComplete = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(complete, decoded);
    }

    #[test]
    fn test_snapshot_request_all_flags() {
        // Test both compressed and uncompressed
        let compressed = SnapshotRequest::compressed();
        assert!(compressed.compressed);
        assert!(compressed.is_fresh_node);

        let uncompressed = SnapshotRequest::uncompressed();
        assert!(!uncompressed.compressed);
        assert!(uncompressed.is_fresh_node);

        // Test non-fresh node flag (edge case)
        let mut not_fresh = SnapshotRequest::compressed();
        not_fresh.is_fresh_node = false;
        assert!(!not_fresh.is_fresh_node);
    }

    // =========================================================================
    // Serialization Edge Cases
    // =========================================================================

    #[test]
    fn test_snapshot_entity_page_with_many_entities_roundtrip() {
        // Test serialization with many entities (but within limit)
        let entities: Vec<SnapshotEntity> = (0..1000)
            .map(|i| make_entity((i % 256) as u8, vec![(i % 256) as u8; 10]))
            .collect();
        let page = SnapshotEntityPage::new(5, 100, entities, false);
        assert!(page.is_valid());

        let encoded = borsh::to_vec(&page).expect("serialize");
        let decoded: SnapshotEntityPage = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(page, decoded);
        assert_eq!(decoded.entity_count(), 1000);
    }

    #[test]
    fn test_snapshot_page_with_large_cursor_roundtrip() {
        let page = SnapshotPage {
            payload: vec![1; 1000],
            uncompressed_len: 5000,
            cursor: Some(vec![0xAB; 256]), // Large cursor
            page_count: 1000,
            sent_count: 500,
        };
        assert!(page.is_valid());

        let encoded = borsh::to_vec(&page).expect("serialize");
        let decoded: SnapshotPage = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(page, decoded);
        assert!(!decoded.is_last());
    }

    #[test]
    fn test_snapshot_verify_result_all_variants_behavior() {
        // Test is_valid returns correctly for all variants
        assert!(SnapshotVerifyResult::Valid.is_valid());
        assert!(!SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32]
        }
        .is_valid());
        assert!(!SnapshotVerifyResult::EntityCountMismatch {
            expected: 100,
            actual: 50
        }
        .is_valid());
        assert!(!SnapshotVerifyResult::MissingPages { missing: vec![1] }.is_valid());

        // Test to_error returns None only for Valid
        assert!(SnapshotVerifyResult::Valid.to_error().is_none());
        assert!(SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32]
        }
        .to_error()
        .is_some());
        assert!(SnapshotVerifyResult::EntityCountMismatch {
            expected: 100,
            actual: 50
        }
        .to_error()
        .is_some());
        assert!(SnapshotVerifyResult::MissingPages { missing: vec![1] }
            .to_error()
            .is_some());
    }

    // =========================================================================
    // Zero-Length Collection Tests
    // =========================================================================

    #[test]
    fn test_snapshot_entity_empty_data() {
        let entity = make_entity(1, vec![]);
        assert!(entity.is_valid());
        assert!(entity.data.is_empty());
    }

    #[test]
    fn test_snapshot_complete_empty_dag_heads() {
        let complete = SnapshotComplete::new([1; 32], 100, 1, 1000);
        assert!(complete.dag_heads.is_empty());
        assert!(complete.is_valid());
    }

    #[test]
    fn test_snapshot_boundary_response_empty_dag_heads() {
        let response = SnapshotBoundaryResponse {
            boundary_timestamp: 12345,
            boundary_root_hash: Hash::default(),
            dag_heads: vec![],
        };
        assert!(response.is_valid());
    }

    #[test]
    fn test_snapshot_verify_result_missing_pages_empty() {
        // Empty missing pages list
        let result = SnapshotVerifyResult::MissingPages { missing: vec![] };
        assert!(!result.is_valid()); // Still invalid even with empty list
        assert!(result.to_error().is_some());
    }

    // =========================================================================
    // Invariant Enforcement Tests
    // =========================================================================

    #[test]
    fn test_invariant_i5_snapshot_safety() {
        // I5: Snapshot ONLY for fresh nodes

        // Fresh node - allowed
        assert!(check_snapshot_safety(false).is_ok());

        // Initialized node - rejected with specific error
        let err = check_snapshot_safety(true).unwrap_err();
        assert!(matches!(err, SnapshotError::SnapshotOnInitializedNode));
    }

    #[test]
    fn test_invariant_i7_verification_errors() {
        // I7: Root hash verification required

        // Hash mismatch should produce RootHashMismatch error
        let result = SnapshotVerifyResult::RootHashMismatch {
            expected: [1; 32],
            computed: [2; 32],
        };
        let error = result.to_error().unwrap();
        match error {
            SnapshotError::RootHashMismatch { expected, computed } => {
                assert_eq!(expected, [1; 32]);
                assert_eq!(computed, [2; 32]);
            }
            _ => panic!("Expected RootHashMismatch error"),
        }
    }

    #[test]
    fn test_snapshot_error_transfer_interrupted_preserves_count() {
        let error = SnapshotError::TransferInterrupted { pages_received: 42 };
        let encoded = borsh::to_vec(&error).expect("serialize");
        let decoded: SnapshotError = borsh::from_slice(&encoded).expect("deserialize");

        match decoded {
            SnapshotError::TransferInterrupted { pages_received } => {
                assert_eq!(pages_received, 42);
            }
            _ => panic!("Expected TransferInterrupted"),
        }
    }

    #[test]
    fn test_snapshot_cursor_roundtrip() {
        let cursor = SnapshotCursor {
            last_key: [0xAB; 32],
        };

        let encoded = borsh::to_vec(&cursor).expect("serialize");
        let decoded: SnapshotCursor = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(cursor, decoded);
        assert_eq!(decoded.last_key, [0xAB; 32]);
    }

    #[test]
    fn test_snapshot_boundary_request_roundtrip() {
        let request = SnapshotBoundaryRequest {
            context_id: ContextId::zero(),
            requested_cutoff_timestamp: Some(1234567890),
        };

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: SnapshotBoundaryRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert_eq!(decoded.requested_cutoff_timestamp, Some(1234567890));

        // Also test with None
        let request_none = SnapshotBoundaryRequest {
            context_id: ContextId::zero(),
            requested_cutoff_timestamp: None,
        };
        let encoded = borsh::to_vec(&request_none).expect("serialize");
        let decoded: SnapshotBoundaryRequest = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(request_none, decoded);
        assert!(decoded.requested_cutoff_timestamp.is_none());
    }
}
