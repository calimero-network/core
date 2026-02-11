//! Side-effect model for sync state machines.
//!
//! Protocol state machines return `SyncActions` effects. The runtime applies them.
//! See spec §3 - Side-Effect Model.

use calimero_primitives::crdt::CrdtType;

use super::runtime::SimDuration;
use super::types::{DeltaId, EntityId, MessageId, NodeId, TimerId, TimerKind};

/// Effects returned by state machine — runtime applies these.
#[derive(Debug, Default)]
pub struct SyncActions {
    /// Messages to send.
    pub messages: Vec<OutgoingMessage>,
    /// Storage operations to apply.
    pub storage_ops: Vec<StorageOp>,
    /// Timer operations.
    pub timer_ops: Vec<TimerOp>,
}

impl SyncActions {
    /// Create empty actions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if there are any actions.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.storage_ops.is_empty() && self.timer_ops.is_empty()
    }

    /// Add a message to send.
    pub fn send(&mut self, to: NodeId, msg: SyncMessage, msg_id: MessageId) {
        self.messages.push(OutgoingMessage { to, msg, msg_id });
    }

    /// Add a storage operation.
    pub fn storage(&mut self, op: StorageOp) {
        self.storage_ops.push(op);
    }

    /// Set a timer.
    pub fn set_timer(&mut self, id: TimerId, delay: SimDuration, kind: TimerKind) {
        self.timer_ops.push(TimerOp::Set { id, delay, kind });
    }

    /// Cancel a timer.
    pub fn cancel_timer(&mut self, id: TimerId) {
        self.timer_ops.push(TimerOp::Cancel { id });
    }

    /// Merge actions from another SyncActions.
    pub fn merge(&mut self, other: SyncActions) {
        self.messages.extend(other.messages);
        self.storage_ops.extend(other.storage_ops);
        self.timer_ops.extend(other.timer_ops);
    }
}

/// Outgoing message with routing info.
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    /// Destination node.
    pub to: NodeId,
    /// The message payload.
    pub msg: SyncMessage,
    /// Unique ID for deduplication (assigned by SM).
    pub msg_id: MessageId,
}

/// Storage operations.
#[derive(Debug, Clone)]
pub enum StorageOp {
    /// Insert a new entity.
    Insert {
        id: EntityId,
        data: Vec<u8>,
        metadata: EntityMetadata,
    },
    /// Update an existing entity.
    Update {
        id: EntityId,
        data: Vec<u8>,
        metadata: EntityMetadata,
    },
    /// Remove an entity (hard delete).
    Remove { id: EntityId },
}

/// Entity metadata for storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityMetadata {
    /// CRDT type for merge semantics.
    pub crdt_type: CrdtType,
    /// HLC timestamp.
    pub hlc_timestamp: u64,
    /// Version counter.
    pub version: u32,
    /// Collection ID (for grouping).
    pub collection_id: [u8; 32],
}

impl Default for EntityMetadata {
    fn default() -> Self {
        Self {
            crdt_type: CrdtType::LwwRegister,
            hlc_timestamp: 0,
            version: 1,
            collection_id: [0; 32],
        }
    }
}

impl EntityMetadata {
    /// Create new metadata with specified CRDT type and timestamp.
    pub fn new(crdt_type: CrdtType, hlc_timestamp: u64) -> Self {
        Self {
            crdt_type,
            hlc_timestamp,
            version: 1,
            collection_id: [0; 32],
        }
    }
}

/// Timer operations.
#[derive(Debug, Clone)]
pub enum TimerOp {
    /// Set a new timer.
    Set {
        id: TimerId,
        delay: SimDuration,
        kind: TimerKind,
    },
    /// Cancel an existing timer.
    Cancel { id: TimerId },
}

/// Sync protocol messages.
///
/// These are the wire-format messages exchanged between nodes during sync.
#[derive(Debug, Clone)]
pub enum SyncMessage {
    // =========================================================================
    // Handshake
    // =========================================================================
    /// Initial handshake from initiator.
    Handshake(HandshakeRequest),
    /// Response to handshake.
    HandshakeResponse(HandshakeResponse),

    // =========================================================================
    // Snapshot Protocol
    // =========================================================================
    /// Request next snapshot page.
    SnapshotRequest { page: u32 },
    /// Snapshot page data.
    SnapshotPage {
        page: u32,
        entities: Vec<EntityTransfer>,
        has_more: bool,
        total_pages: u32,
    },
    /// Snapshot complete acknowledgment.
    SnapshotComplete {
        success: bool,
        error: Option<String>,
    },

    // =========================================================================
    // HashComparison Protocol
    // =========================================================================
    /// Request children of a Merkle node.
    HashCompareRequest { node_hash: [u8; 32], level: u32 },
    /// Children hashes response.
    HashCompareResponse {
        node_hash: [u8; 32],
        children: Vec<([u8; 32], bool)>, // (hash, is_leaf)
        has_more: bool,
    },
    /// Request entity data by ID.
    EntityRequest { ids: Vec<EntityId> },
    /// Entity data response.
    EntityResponse { entities: Vec<EntityTransfer> },

    // =========================================================================
    // DeltaSync Protocol
    // =========================================================================
    /// Advertise DAG heads.
    DeltaHeads { heads: Vec<DeltaId> },
    /// Request missing deltas.
    DeltaRequest { ids: Vec<DeltaId> },
    /// Delta data response.
    DeltaResponse { deltas: Vec<DeltaTransfer> },

    // =========================================================================
    // Common
    // =========================================================================
    /// Sync complete (from either side).
    SyncComplete { success: bool },
    /// Error during sync.
    SyncError { code: u32, message: String },
}

impl SyncMessage {
    /// Estimate the total size of this message including heap allocations.
    ///
    /// This provides a rough approximation of the wire-format size for metrics.
    pub fn estimated_size(&self) -> usize {
        use std::mem::size_of;

        // Base stack size of the enum
        let base = size_of::<Self>();

        // Add heap allocation sizes for each variant
        let heap_size = match self {
            SyncMessage::Handshake(req) => req.dag_heads.len() * size_of::<DeltaId>(),
            SyncMessage::HandshakeResponse(resp) => resp.reason.len(),
            SyncMessage::SnapshotRequest { .. } => 0,
            SyncMessage::SnapshotPage { entities, .. } => entities
                .iter()
                .map(|e| e.data.len() + size_of::<EntityTransfer>())
                .sum(),
            SyncMessage::SnapshotComplete { error, .. } => {
                error.as_ref().map(|s| s.len()).unwrap_or(0)
            }
            SyncMessage::HashCompareRequest { .. } => 0,
            SyncMessage::HashCompareResponse { children, .. } => {
                children.len() * size_of::<([u8; 32], bool)>()
            }
            SyncMessage::EntityRequest { ids } => ids.len() * size_of::<EntityId>(),
            SyncMessage::EntityResponse { entities } => entities
                .iter()
                .map(|e| e.data.len() + size_of::<EntityTransfer>())
                .sum(),
            SyncMessage::DeltaHeads { heads } => heads.len() * size_of::<DeltaId>(),
            SyncMessage::DeltaRequest { ids } => ids.len() * size_of::<DeltaId>(),
            SyncMessage::DeltaResponse { deltas } => deltas
                .iter()
                .map(|d| {
                    d.parents.len() * size_of::<DeltaId>()
                        + d.operations
                            .iter()
                            .map(|op| match op {
                                StorageOp::Insert { data, .. } | StorageOp::Update { data, .. } => {
                                    data.len() + size_of::<StorageOp>()
                                }
                                StorageOp::Remove { .. } => size_of::<StorageOp>(),
                            })
                            .sum::<usize>()
                        + size_of::<DeltaTransfer>()
                })
                .sum(),
            SyncMessage::SyncComplete { .. } => 0,
            SyncMessage::SyncError { message, .. } => message.len(),
        };

        base + heap_size
    }
}

/// Handshake request (initiator → responder).
#[derive(Debug, Clone)]
pub struct HandshakeRequest {
    /// Protocol version.
    pub version: u32,
    /// Current Merkle root hash.
    pub root_hash: [u8; 32],
    /// Number of entities.
    pub entity_count: u64,
    /// Maximum tree depth.
    pub max_depth: u32,
    /// Current DAG heads.
    pub dag_heads: Vec<DeltaId>,
    /// Whether this node has any state.
    pub has_state: bool,
}

/// Handshake response (responder → initiator).
#[derive(Debug, Clone)]
pub struct HandshakeResponse {
    /// Selected protocol.
    pub protocol: SelectedProtocol,
    /// Responder's root hash.
    pub root_hash: [u8; 32],
    /// Responder's entity count.
    pub entity_count: u64,
    /// Reason for protocol selection (for debugging).
    pub reason: String,
}

/// Selected sync protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum SelectedProtocol {
    /// No sync needed.
    None,
    /// Full snapshot transfer.
    Snapshot { compressed: bool },
    /// Hash-based comparison.
    HashComparison,
    /// Delta-based sync.
    DeltaSync { missing_count: usize },
    /// Bloom filter sync.
    BloomFilter { filter_size: u64 },
    /// Subtree prefetch.
    SubtreePrefetch,
    /// Level-wise sync.
    LevelWise { max_depth: u32 },
}

/// Entity data for transfer.
#[derive(Debug, Clone)]
pub struct EntityTransfer {
    /// Entity ID.
    pub id: EntityId,
    /// Entity data.
    pub data: Vec<u8>,
    /// Entity metadata.
    pub metadata: EntityMetadata,
}

/// Delta data for transfer.
#[derive(Debug, Clone)]
pub struct DeltaTransfer {
    /// Delta ID.
    pub id: DeltaId,
    /// Parent delta IDs.
    pub parents: Vec<DeltaId>,
    /// Operations in this delta.
    pub operations: Vec<StorageOp>,
    /// HLC timestamp.
    pub hlc_timestamp: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_actions_empty() {
        let actions = SyncActions::new();
        assert!(actions.is_empty());
    }

    #[test]
    fn test_sync_actions_send() {
        let mut actions = SyncActions::new();

        actions.send(
            NodeId::new("bob"),
            SyncMessage::SyncComplete { success: true },
            MessageId::new("alice", 1, 1),
        );

        assert!(!actions.is_empty());
        assert_eq!(actions.messages.len(), 1);
        assert_eq!(actions.messages[0].to.as_str(), "bob");
    }

    #[test]
    fn test_sync_actions_storage() {
        let mut actions = SyncActions::new();

        actions.storage(StorageOp::Insert {
            id: EntityId::from_u64(1),
            data: vec![1, 2, 3],
            metadata: EntityMetadata::default(),
        });

        assert!(!actions.is_empty());
        assert_eq!(actions.storage_ops.len(), 1);
    }

    #[test]
    fn test_sync_actions_timer() {
        let mut actions = SyncActions::new();

        actions.set_timer(
            TimerId::new(1),
            SimDuration::from_millis(100),
            TimerKind::Sync,
        );
        actions.cancel_timer(TimerId::new(2));

        assert!(!actions.is_empty());
        assert_eq!(actions.timer_ops.len(), 2);
    }

    #[test]
    fn test_sync_actions_merge() {
        let mut a = SyncActions::new();
        a.send(
            NodeId::new("bob"),
            SyncMessage::SyncComplete { success: true },
            MessageId::new("alice", 1, 1),
        );

        let mut b = SyncActions::new();
        b.storage(StorageOp::Remove {
            id: EntityId::from_u64(1),
        });

        a.merge(b);

        assert_eq!(a.messages.len(), 1);
        assert_eq!(a.storage_ops.len(), 1);
    }
}
