//! Simulated node state.
//!
//! Wraps storage, DAG, and sync state machine.

use std::collections::{HashMap, HashSet};

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
    /// Sync state machine state.
    pub sync_state: SyncState,
    /// Active timers.
    pub timers: Vec<TimerEntry>,
    /// Next timer ID.
    next_timer_id: u64,
    /// Processed message IDs (for deduplication).
    processed_messages: HashSet<MessageId>,
    /// Last seen session per sender (for stale message detection).
    sender_sessions: HashMap<String, u64>,
    /// Maximum size of processed_messages before eviction.
    max_processed_messages: usize,
    /// Whether node has been initialized (ever had state).
    pub has_state: bool,
}

impl SimNode {
    /// Create a new node.
    pub fn new(id: impl Into<NodeId>) -> Self {
        Self {
            id: id.into(),
            session: 0,
            out_seq: 0,
            storage: DigestCache::new(),
            dag_heads: vec![DeltaId::ZERO],
            delta_buffer: Vec::new(),
            sync_state: SyncState::Idle,
            timers: Vec::new(),
            next_timer_id: 0,
            processed_messages: HashSet::new(),
            sender_sessions: HashMap::new(),
            max_processed_messages: 10000,
            has_state: false,
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
    pub fn is_duplicate(&self, msg_id: &MessageId) -> bool {
        // Check if message is from a stale session of the sender
        if let Some(&known_session) = self.sender_sessions.get(&msg_id.sender) {
            if msg_id.session < known_session {
                return true;
            }
        }
        self.processed_messages.contains(msg_id)
    }

    /// Mark message as processed.
    pub fn mark_processed(&mut self, msg_id: MessageId) {
        // Update the sender's known session
        let sender_session = self
            .sender_sessions
            .entry(msg_id.sender.clone())
            .or_insert(0);
        if msg_id.session > *sender_session {
            *sender_session = msg_id.session;
        }

        // Evict oldest entries if we're at capacity
        if self.processed_messages.len() >= self.max_processed_messages {
            // Simple eviction: clear half the entries
            // In a real implementation, we'd use LRU, but for simulation this is sufficient
            let to_remove: Vec<_> = self
                .processed_messages
                .iter()
                .take(self.max_processed_messages / 2)
                .cloned()
                .collect();
            for id in to_remove {
                self.processed_messages.remove(&id);
            }
        }

        self.processed_messages.insert(msg_id);
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
    pub fn buffer_delta(&mut self, delta_id: DeltaId) {
        if !self.delta_buffer.contains(&delta_id) {
            self.delta_buffer.push(delta_id);
        }
    }

    /// Clear delta buffer.
    pub fn clear_buffer(&mut self) {
        self.delta_buffer.clear();
    }

    /// Crash the node (see spec §6).
    pub fn crash(&mut self) {
        // Preserve: storage, DAG (dag_heads)
        // Lose: timers, sync state, buffer, processed messages, out_seq

        self.timers.clear();
        self.sync_state = SyncState::Idle;
        self.delta_buffer.clear();
        self.processed_messages.clear();
        self.sender_sessions.clear();
        self.out_seq = 0;
    }

    /// Restart the node after crash.
    pub fn restart(&mut self) {
        // Increment session
        self.session += 1;
        // out_seq already reset in crash()
        // State machine already Idle from crash()
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

        // Stale sender session - message from sender's old session
        // First, process a message from bob's session 1
        let new_msg = MessageId::new("bob", 1, 0);
        node.mark_processed(new_msg);
        // Now a message from bob's session 0 should be stale
        let old_msg = MessageId::new("bob", 0, 5);
        assert!(node.is_duplicate(&old_msg)); // Sender's old session
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
