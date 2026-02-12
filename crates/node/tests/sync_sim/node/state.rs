//! Simulated node state.
//!
//! Wraps storage, DAG, and sync state machine.
//!
//! `SimNode` implements `LocalSyncState` trait from `calimero_node_primitives::sync`
//! to share protocol logic with production `SyncManager`.

use std::collections::{HashMap, HashSet, VecDeque};

use calimero_node_primitives::sync::handshake::SyncHandshake;
use calimero_node_primitives::sync::state_machine::{build_handshake, LocalSyncState};
use calimero_primitives::crdt::CrdtType;

use crate::sync_sim::actions::{EntityMetadata, StorageOp};
use crate::sync_sim::digest::{DigestCache, DigestEntity};
use crate::sync_sim::runtime::SimTime;
use crate::sync_sim::types::{
    DeltaId, EntityId, MessageId, NodeId, StateDigest, TimerId, TimerKind,
};

/// Sync state machine state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncState {
    /// Not syncing.
    Idle,
    /// Initiating sync with a peer.
    Initiating { peer: NodeId },
    /// Responding to sync from a peer.
    Responding { peer: NodeId },
    /// Snapshot transfer in progress.
    SnapshotTransfer { peer: NodeId, page: u32 },
    /// Hash comparison in progress.
    HashComparison { peer: NodeId, level: u32 },
    /// Delta sync in progress.
    DeltaSync { peer: NodeId },
}

impl SyncState {
    /// Check if idle.
    pub fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Check if syncing.
    pub fn is_active(&self) -> bool {
        !self.is_idle()
    }

    /// Get peer if syncing.
    pub fn peer(&self) -> Option<&NodeId> {
        match self {
            Self::Idle => None,
            Self::Initiating { peer }
            | Self::Responding { peer }
            | Self::SnapshotTransfer { peer, .. }
            | Self::HashComparison { peer, .. }
            | Self::DeltaSync { peer } => Some(peer),
        }
    }
}

/// Timer entry.
#[derive(Debug, Clone)]
pub struct TimerEntry {
    /// Timer ID.
    pub id: TimerId,
    /// Timer kind.
    pub kind: TimerKind,
    /// Scheduled fire time.
    pub fire_time: SimTime,
}

/// Maximum number of processed messages to track before eviction.
const MAX_PROCESSED_MESSAGES: usize = 10_000;

/// Default buffer capacity for delta buffering during sync.
/// Matches production default from `calimero_node_primitives::delta_buffer::DEFAULT_BUFFER_CAPACITY`.
pub const DEFAULT_SIM_BUFFER_CAPACITY: usize = 10_000;

/// Simulated node.
#[derive(Debug)]
pub struct SimNode {
    /// Node ID.
    pub id: NodeId,
    /// Current session (increments on restart).
    pub session: u64,
    /// Outgoing message sequence counter.
    pub out_seq: u64,
    /// Storage with digest caching.
    pub storage: DigestCache,
    /// DAG heads.
    pub dag_heads: Vec<DeltaId>,
    /// Delta buffer (references to DAG entries).
    pub delta_buffer: Vec<DeltaId>,
    /// Buffered operations for replay after sync completes (Invariant I6).
    /// Maps delta_id -> operations to apply.
    buffered_operations: HashMap<DeltaId, Vec<crate::sync_sim::actions::StorageOp>>,
    /// Buffer capacity (for FIFO eviction testing).
    buffer_capacity: usize,
    /// Number of deltas dropped due to buffer overflow.
    buffer_drops: u64,
    /// Sync state machine state.
    pub sync_state: SyncState,
    /// Active timers.
    pub timers: Vec<TimerEntry>,
    /// Next timer ID.
    next_timer_id: u64,
    /// Processed message IDs (for deduplication), bounded to MAX_PROCESSED_MESSAGES.
    processed_messages: HashSet<MessageId>,
    /// Ordered deque for O(1) LRU-like eviction of processed messages.
    processed_order: VecDeque<MessageId>,
    /// Highest session seen from each sender (for stale message detection).
    sender_sessions: HashMap<String, u64>,
    /// Whether node has been initialized (ever had state).
    pub has_state: bool,
    /// Whether node is currently crashed (offline).
    pub is_crashed: bool,
}

impl SimNode {
    /// Create a new node with default buffer capacity.
    pub fn new(id: impl Into<NodeId>) -> Self {
        Self::with_buffer_capacity(id, DEFAULT_SIM_BUFFER_CAPACITY)
    }

    /// Create a new node with custom buffer capacity.
    ///
    /// Useful for testing buffer overflow behavior (Invariant I6).
    pub fn with_buffer_capacity(id: impl Into<NodeId>, buffer_capacity: usize) -> Self {
        Self {
            id: id.into(),
            session: 0,
            out_seq: 0,
            storage: DigestCache::new(),
            dag_heads: vec![DeltaId::ZERO],
            delta_buffer: Vec::new(),
            buffered_operations: HashMap::new(),
            buffer_capacity,
            buffer_drops: 0,
            sync_state: SyncState::Idle,
            timers: Vec::new(),
            next_timer_id: 0,
            processed_messages: HashSet::new(),
            processed_order: VecDeque::new(),
            sender_sessions: HashMap::new(),
            has_state: false,
            is_crashed: false,
        }
    }

    /// Get node ID.
    pub fn id(&self) -> &NodeId {
        &self.id
    }

    /// Generate next message ID.
    pub fn next_message_id(&mut self) -> MessageId {
        let id = MessageId::new(self.id.as_str(), self.session, self.out_seq);
        self.out_seq += 1;
        id
    }

    /// Generate next timer ID.
    pub fn next_timer_id(&mut self) -> TimerId {
        let id = TimerId::new(self.next_timer_id);
        self.next_timer_id += 1;
        id
    }

    /// Check if message was already processed.
    ///
    /// A message is considered duplicate if:
    /// 1. We've seen a newer session from this sender (message is stale), OR
    /// 2. We've already processed this exact message ID
    pub fn is_duplicate(&self, msg_id: &MessageId) -> bool {
        // Check if we've seen a newer session from this sender
        if let Some(&last_session) = self.sender_sessions.get(&msg_id.sender) {
            if msg_id.session < last_session {
                // Stale session from this sender
                return true;
            }
        }
        self.processed_messages.contains(msg_id)
    }

    /// Mark message as processed.
    ///
    /// Uses bounded storage with LRU-like eviction when MAX_PROCESSED_MESSAGES is reached.
    pub fn mark_processed(&mut self, msg_id: MessageId) {
        // Update the highest session seen from this sender
        let current = self
            .sender_sessions
            .entry(msg_id.sender.clone())
            .or_insert(0);
        if msg_id.session > *current {
            *current = msg_id.session;
        }

        // Only add if not already present
        if self.processed_messages.insert(msg_id.clone()) {
            self.processed_order.push_back(msg_id);

            // Evict oldest entries if over limit (O(1) with VecDeque::pop_front)
            while self.processed_messages.len() > MAX_PROCESSED_MESSAGES {
                if let Some(oldest) = self.processed_order.pop_front() {
                    self.processed_messages.remove(&oldest);
                } else {
                    break;
                }
            }
        }
    }

    /// Get state digest.
    pub fn state_digest(&mut self) -> StateDigest {
        self.storage.digest()
    }

    /// Get entity count.
    pub fn entity_count(&self) -> usize {
        self.storage.len()
    }

    /// Get root hash (for handshake).
    pub fn root_hash(&mut self) -> [u8; 32] {
        self.storage.digest().0
    }

    /// Check if node has any state.
    pub fn has_any_state(&self) -> bool {
        self.has_state || !self.storage.is_empty()
    }

    /// Get DAG heads.
    pub fn dag_heads(&self) -> &[DeltaId] {
        &self.dag_heads
    }

    /// Get buffer size.
    pub fn buffer_size(&self) -> usize {
        self.delta_buffer.len()
    }

    /// Get sync timer count.
    pub fn sync_timer_count(&self) -> usize {
        self.timers
            .iter()
            .filter(|t| t.kind == TimerKind::Sync)
            .count()
    }

    /// Apply storage operation.
    pub fn apply_storage_op(&mut self, op: StorageOp) {
        match op {
            StorageOp::Insert { id, data, metadata } | StorageOp::Update { id, data, metadata } => {
                self.storage.upsert(DigestEntity { id, data, metadata });
                self.has_state = true;
            }
            StorageOp::Remove { id } => {
                self.storage.remove(&id);
            }
        }
    }

    /// Set a timer.
    pub fn set_timer(&mut self, id: TimerId, fire_time: SimTime, kind: TimerKind) {
        // Remove any existing timer with same ID
        self.timers.retain(|t| t.id != id);
        self.timers.push(TimerEntry {
            id,
            kind,
            fire_time,
        });
    }

    /// Cancel a timer.
    pub fn cancel_timer(&mut self, id: TimerId) {
        self.timers.retain(|t| t.id != id);
    }

    /// Get timer by ID.
    pub fn get_timer(&self, id: TimerId) -> Option<&TimerEntry> {
        self.timers.iter().find(|t| t.id == id)
    }

    /// Buffer a delta (during sync).
    ///
    /// Implements FIFO eviction when buffer is full. Returns `true` if added without
    /// eviction, `false` if oldest delta was evicted or dropped.
    pub fn buffer_delta(&mut self, delta_id: DeltaId) -> bool {
        // Handle zero capacity: drop incoming delta immediately
        // (matches production DeltaBuffer::push behavior)
        if self.buffer_capacity == 0 {
            self.buffer_drops += 1;
            return false;
        }

        // Don't buffer duplicates
        if self.delta_buffer.contains(&delta_id) {
            return true;
        }

        // Check capacity - evict oldest if full
        if self.delta_buffer.len() >= self.buffer_capacity {
            // Evict oldest delta (front of buffer) - FIFO policy
            if let Some(evicted_id) = self.delta_buffer.first().copied() {
                self.delta_buffer.remove(0);
                // Also remove its buffered operations
                self.buffered_operations.remove(&evicted_id);
                self.buffer_drops += 1;
            }
            self.delta_buffer.push(delta_id);
            false // Added with eviction
        } else {
            self.delta_buffer.push(delta_id);
            true // Added without eviction
        }
    }

    /// Clear delta buffer.
    pub fn clear_buffer(&mut self) {
        self.delta_buffer.clear();
    }

    /// Get buffer drops count.
    pub fn buffer_drops(&self) -> u64 {
        self.buffer_drops
    }

    /// Set buffer capacity (for testing overflow scenarios).
    pub fn set_buffer_capacity(&mut self, capacity: usize) {
        self.buffer_capacity = capacity;
    }

    /// Buffer operations for a delta (for replay after sync completes).
    ///
    /// This implements Invariant I6: deltas arriving during sync are buffered
    /// and replayed after sync completes.
    pub fn buffer_operations(
        &mut self,
        delta_id: DeltaId,
        operations: Vec<crate::sync_sim::actions::StorageOp>,
    ) {
        self.buffered_operations.insert(delta_id, operations);
    }

    /// Drain all buffered operations for replay.
    ///
    /// Returns operations in FIFO order (based on delta_buffer order).
    ///
    /// # Debug assertions
    ///
    /// In debug builds, asserts that there are no orphaned operations (operations
    /// whose delta_id is not in the delta_buffer). This would indicate a bug in
    /// the buffering logic.
    pub fn drain_buffered_operations(
        &mut self,
    ) -> Vec<(DeltaId, Vec<crate::sync_sim::actions::StorageOp>)> {
        // Return in FIFO order based on delta_buffer
        let mut result = Vec::new();
        for delta_id in &self.delta_buffer {
            if let Some(ops) = self.buffered_operations.remove(delta_id) {
                result.push((*delta_id, ops));
            }
        }

        // Check for orphaned operations (bug indicator)
        let orphan_count = self.buffered_operations.len();
        debug_assert!(
            orphan_count == 0,
            "Found {} orphaned buffered operations (delta_id not in delta_buffer). \
             This indicates a bug in the buffering logic.",
            orphan_count
        );

        // Clear any remaining orphaned operations (graceful degradation in release)
        self.buffered_operations.clear();
        result
    }

    /// Get count of buffered operations.
    pub fn buffered_operations_count(&self) -> usize {
        self.buffered_operations.len()
    }

    /// Finish sync session: replay buffered operations, clear buffers, reset state.
    ///
    /// This atomically performs all cleanup needed after sync completes:
    /// 1. Drains and applies all buffered operations
    /// 2. Clears the delta ID buffer
    /// 3. Resets sync state to Idle
    ///
    /// Returns the number of storage operations applied (for metrics).
    pub fn finish_sync(&mut self) -> usize {
        if !self.sync_state.is_active() {
            return 0;
        }

        // Replay buffered deltas
        let buffered_ops = self.drain_buffered_operations();
        let mut ops_applied = 0;
        for (_delta_id, operations) in buffered_ops {
            for op in operations {
                self.apply_storage_op(op);
                ops_applied += 1;
            }
        }

        // Clear the delta ID buffer
        self.clear_buffer();

        // Transition to idle
        self.sync_state = SyncState::Idle;

        ops_applied
    }

    /// Crash the node (see spec §6).
    pub fn crash(&mut self) {
        // Preserve: storage, DAG (dag_heads)
        // Lose: timers, sync state, buffer, buffered operations, processed messages, sender sessions, out_seq

        self.timers.clear();
        self.sync_state = SyncState::Idle;
        self.delta_buffer.clear();
        self.buffered_operations.clear();
        self.processed_messages.clear();
        self.processed_order.clear();
        self.sender_sessions.clear();
        self.out_seq = 0;
        self.is_crashed = true;
    }

    /// Restart the node after crash.
    pub fn restart(&mut self) {
        // Increment session
        self.session += 1;
        // out_seq already reset in crash()
        // State machine already Idle from crash()
        self.is_crashed = false;
    }

    /// Insert entity directly (for test setup).
    pub fn insert_entity(&mut self, id: EntityId, data: Vec<u8>, crdt_type: CrdtType) {
        let metadata = EntityMetadata::new(crdt_type, 0);
        self.storage.upsert(DigestEntity { id, data, metadata });
        self.has_state = true;
    }

    /// Insert entity with full metadata.
    pub fn insert_entity_with_metadata(
        &mut self,
        id: EntityId,
        data: Vec<u8>,
        metadata: EntityMetadata,
    ) {
        self.storage.upsert(DigestEntity { id, data, metadata });
        self.has_state = true;
    }

    /// Get entity.
    pub fn get_entity(&self, id: &EntityId) -> Option<&DigestEntity> {
        self.storage.get(id)
    }

    /// Check if entity exists.
    pub fn has_entity(&self, id: &EntityId) -> bool {
        self.storage.get(id).is_some()
    }

    /// Build a SyncHandshake for this node.
    ///
    /// Uses the shared `build_handshake()` function from `calimero_node_primitives::sync`
    /// to ensure consistent behavior between simulation and production.
    ///
    /// Note: Unlike `SyncManager` which estimates entity_count from dag_heads,
    /// `SimNode` provides the actual entity count from storage. This gives more
    /// accurate protocol selection in simulation while using the same selection logic.
    pub fn build_handshake(&mut self) -> SyncHandshake {
        // Use the shared build_handshake function via LocalSyncState trait
        build_handshake(self)
    }
}

// =============================================================================
// LocalSyncState Implementation
// =============================================================================

/// Implement `LocalSyncState` to enable shared protocol logic between
/// `SimNode` (simulation) and `SyncManager` (production).
///
/// This ensures that:
/// 1. Protocol selection uses the same `select_protocol()` function
/// 2. Handshake building follows the same structure
/// 3. Both environments can be tested with the same test scenarios
///
/// Note: `root_hash()` computes the digest fresh (without caching) to satisfy
/// the `&self` requirement of the trait. This is acceptable for protocol
/// negotiation which happens infrequently.
impl LocalSyncState for SimNode {
    fn root_hash(&self) -> [u8; 32] {
        // Compute digest without using cache (trait requires &self, cache needs &mut self)
        // This is acceptable for protocol negotiation which is infrequent
        use crate::sync_sim::digest::{compute_state_digest, DigestEntity};
        let entities: Vec<DigestEntity> = self.storage.iter().cloned().collect();
        compute_state_digest(&entities).0
    }

    fn entity_count(&self) -> u64 {
        // Use actual storage count for precise simulation
        self.storage.len() as u64
    }

    fn max_depth(&self) -> u32 {
        // Estimate max_depth from entity count (log2-ish for balanced tree)
        let count = self.entity_count();
        if count == 0 {
            0
        } else {
            (64u32 - count.leading_zeros()).min(32)
        }
    }

    fn dag_heads(&self) -> Vec<[u8; 32]> {
        self.dag_heads.iter().map(|d| d.0).collect()
    }

    fn has_state(&self) -> bool {
        // Use explicit has_state flag for simulation consistency
        self.has_state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_creation() {
        let node = SimNode::new("alice");

        assert_eq!(node.id().as_str(), "alice");
        assert_eq!(node.session, 0);
        assert!(node.sync_state.is_idle());
        assert_eq!(node.entity_count(), 0);
        assert!(!node.has_any_state());
    }

    #[test]
    fn test_message_id_generation() {
        let mut node = SimNode::new("alice");

        let m1 = node.next_message_id();
        let m2 = node.next_message_id();

        assert_eq!(m1.sender, "alice");
        assert_eq!(m1.session, 0);
        assert_eq!(m1.seq, 0);

        assert_eq!(m2.seq, 1);
    }

    #[test]
    fn test_duplicate_detection() {
        let mut node = SimNode::new("alice");

        let msg_id = MessageId::new("bob", 0, 1);

        assert!(!node.is_duplicate(&msg_id));
        node.mark_processed(msg_id.clone());
        assert!(node.is_duplicate(&msg_id));
    }

    #[test]
    fn test_duplicate_detection_sender_session() {
        let mut node = SimNode::new("alice");

        // Process a message from bob's session 1
        let msg_session1 = MessageId::new("bob", 1, 0);
        assert!(!node.is_duplicate(&msg_session1));
        node.mark_processed(msg_session1);

        // A message from bob's older session 0 should be stale
        let old_msg = MessageId::new("bob", 0, 5);
        assert!(node.is_duplicate(&old_msg)); // Stale sender session

        // A message from bob's current session 1 (different seq) should not be duplicate
        let msg_session1_seq1 = MessageId::new("bob", 1, 1);
        assert!(!node.is_duplicate(&msg_session1_seq1));

        // A message from charlie should be independent
        let charlie_msg = MessageId::new("charlie", 0, 0);
        assert!(!node.is_duplicate(&charlie_msg));
    }

    #[test]
    fn test_storage_operations() {
        let mut node = SimNode::new("alice");

        let id = EntityId::from_u64(1);
        node.insert_entity(id, vec![1, 2, 3], CrdtType::LwwRegister);

        assert!(node.has_any_state());
        assert_eq!(node.entity_count(), 1);
        assert!(node.has_entity(&id));

        let entity = node.get_entity(&id).unwrap();
        assert_eq!(entity.data, vec![1, 2, 3]);
    }

    #[test]
    fn test_crash_restart() {
        let mut node = SimNode::new("alice");

        // Set up some state
        node.insert_entity(EntityId::from_u64(1), vec![1], CrdtType::LwwRegister);
        node.sync_state = SyncState::Initiating {
            peer: NodeId::new("bob"),
        };
        node.out_seq = 10;
        node.set_timer(TimerId::new(1), SimTime::from_millis(100), TimerKind::Sync);

        // Crash
        node.crash();

        // Preserved: storage
        assert_eq!(node.entity_count(), 1);

        // Lost
        assert!(node.sync_state.is_idle());
        assert!(node.timers.is_empty());
        assert_eq!(node.out_seq, 0);

        // Restart
        node.restart();
        assert_eq!(node.session, 1);
    }

    #[test]
    fn test_timers() {
        let mut node = SimNode::new("alice");

        let t1 = node.next_timer_id();
        let t2 = node.next_timer_id();

        node.set_timer(t1, SimTime::from_millis(100), TimerKind::Sync);
        node.set_timer(t2, SimTime::from_millis(200), TimerKind::Housekeeping);

        assert_eq!(node.sync_timer_count(), 1);
        assert!(node.get_timer(t1).is_some());

        node.cancel_timer(t1);
        assert!(node.get_timer(t1).is_none());
        assert_eq!(node.sync_timer_count(), 0);
    }

    #[test]
    fn test_delta_buffer() {
        let mut node = SimNode::new("alice");

        assert_eq!(node.buffer_size(), 0);

        node.buffer_delta(DeltaId::from_bytes([1; 32]));
        node.buffer_delta(DeltaId::from_bytes([2; 32]));
        node.buffer_delta(DeltaId::from_bytes([1; 32])); // Duplicate

        assert_eq!(node.buffer_size(), 2);

        node.clear_buffer();
        assert_eq!(node.buffer_size(), 0);
    }

    #[test]
    fn test_state_digest() {
        let mut node = SimNode::new("alice");

        let d1 = node.state_digest();
        assert_eq!(d1, StateDigest::ZERO);

        node.insert_entity(EntityId::from_u64(1), vec![1], CrdtType::LwwRegister);

        let d2 = node.state_digest();
        assert_ne!(d2, StateDigest::ZERO);
        assert_ne!(d2, d1);
    }
}
