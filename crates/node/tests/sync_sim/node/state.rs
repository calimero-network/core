//! Simulated node state.
//!
//! Wraps storage, DAG, and sync state machine.
//!
//! See [Simulation Framework Spec](https://github.com/calimero-network/specs/blob/main/sync/simulation-framework.md):
//! - §2: Architecture Overview
//! - §3: Side-Effect Model (`SyncActions`)
//! - §6: Crash/Restart Semantics
//!
//! # Storage Architecture (Spec §5, §7)
//!
//! `SimNode` uses a hybrid storage approach:
//! - `SimStorage`: Real Merkle tree backed by `calimero-storage::Index<MainStorage>`
//!   for accurate tree structure, hash propagation, and sync protocol testing.
//! - `entity_metadata`: Simulation-specific metadata cache (`HashMap<EntityId, EntityMetadata>`)
//!   since the storage layer's `Metadata` type doesn't have all simulation fields (CrdtType, etc.)
//!
//! This allows testing real Merkle tree behavior while maintaining backward-compatible APIs.
//!
//! # LocalSyncState Implementation
//!
//! `SimNode` implements `LocalSyncState` trait from `calimero_node_primitives::sync`
//! to share protocol logic with production `SyncManager`. This ensures:
//! - Protocol selection uses the same `select_protocol()` function
//! - Handshake building follows the same structure
//! - Both environments can be tested with the same test scenarios

use std::collections::{HashMap, HashSet, VecDeque};

use calimero_node_primitives::sync::handshake::SyncHandshake;
use calimero_node_primitives::sync::protocol::SyncProtocol;
use calimero_node_primitives::sync::state_machine::{build_handshake, LocalSyncState};

use crate::sync_sim::actions::SelectedProtocol;
use calimero_primitives::context::ContextId;
use calimero_primitives::crdt::CrdtType;
use calimero_primitives::identity::PublicKey;
use calimero_storage::address::Id;
use calimero_storage::entities::Metadata;

use crate::sync_sim::actions::{EntityMetadata, StorageOp};
use crate::sync_sim::digest::DigestEntity;
use crate::sync_sim::runtime::SimTime;
use crate::sync_sim::storage::SimStorage;
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

/// Maximum depth for hierarchical entity insertion.
///
/// Key bytes 0..24 are used for tree depth, bytes 24..32 are reserved for
/// seed/uniqueness in test entity generation. This matches the key layout
/// in `generate_deep_tree_entities()`.
const MAX_HIERARCHICAL_DEPTH: usize = 24;

/// Simulated node.
///
/// Uses real Merkle tree storage (`SimStorage`) for accurate sync protocol testing
/// while maintaining simulation-specific metadata in a separate cache.
#[derive(Debug)]
pub struct SimNode {
    /// Node ID.
    pub id: NodeId,
    /// Current session (increments on restart).
    pub session: u64,
    /// Outgoing message sequence counter.
    pub out_seq: u64,
    /// Merkle tree storage backed by real `calimero-storage` implementation.
    storage: SimStorage,
    /// Simulation-specific entity metadata cache.
    /// Stored separately because `calimero-storage::Metadata` doesn't include
    /// simulation fields like `CrdtType`, `hlc_timestamp`, `version`, `collection_id`.
    entity_metadata: HashMap<EntityId, EntityMetadata>,
    /// DAG heads.
    pub dag_heads: Vec<DeltaId>,
    /// Delta buffer (references to DAG entries).
    /// Uses VecDeque for O(1) FIFO eviction (#5: performance fix).
    pub delta_buffer: VecDeque<DeltaId>,
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
    /// Forced protocol override for testing.
    /// When set, `select_protocol_for_sync()` returns this instead of auto-selecting.
    forced_protocol: Option<SelectedProtocol>,
}

impl SimNode {
    /// Default shared context ID for simulation.
    /// All nodes in a simulation typically share this context.
    pub const DEFAULT_CONTEXT_ID: [u8; 32] = [0xCA; 32]; // "Calimero"

    /// Create a new node with default buffer capacity.
    ///
    /// Creates unique `ContextId` and `PublicKey` derived from the node ID
    /// for isolated storage instances.
    pub fn new(id: impl Into<NodeId>) -> Self {
        Self::with_buffer_capacity(id, DEFAULT_SIM_BUFFER_CAPACITY)
    }

    /// Create a new node in a shared context.
    ///
    /// Nodes that sync together should share the same context ID.
    /// This is the correct way to create nodes for sync testing.
    pub fn new_in_context(id: impl Into<NodeId>, context_id: ContextId) -> Self {
        Self::with_context_and_buffer(id, context_id, DEFAULT_SIM_BUFFER_CAPACITY)
    }

    /// Create a new node with custom buffer capacity.
    ///
    /// Useful for testing buffer overflow behavior (Invariant I6).
    /// Creates unique `ContextId` and `PublicKey` derived from the node ID
    /// for isolated storage instances.
    pub fn with_buffer_capacity(id: impl Into<NodeId>, buffer_capacity: usize) -> Self {
        let node_id = id.into();

        // Create deterministic context/executor IDs from node name
        let context_id = Self::create_context_id(&node_id);
        Self::with_context_and_buffer(node_id, context_id, buffer_capacity)
    }

    /// Create a new node with specific context and buffer capacity.
    ///
    /// This is the most flexible constructor, allowing full control over
    /// context ID (for shared contexts) and buffer capacity (for I6 testing).
    pub fn with_context_and_buffer(
        id: impl Into<NodeId>,
        context_id: ContextId,
        buffer_capacity: usize,
    ) -> Self {
        let node_id = id.into();
        let executor_id = Self::create_executor_id(&node_id);

        Self {
            id: node_id,
            session: 0,
            out_seq: 0,
            storage: SimStorage::new(context_id, executor_id),
            entity_metadata: HashMap::new(),
            dag_heads: vec![DeltaId::ZERO],
            delta_buffer: VecDeque::new(), // VecDeque for O(1) FIFO eviction
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
            forced_protocol: None,
        }
    }

    /// Create a deterministic ContextId from node name.
    fn create_context_id(node_id: &NodeId) -> ContextId {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"sim_context:");
        hasher.update(node_id.as_str().as_bytes());
        let hash: [u8; 32] = hasher.finalize().into();
        ContextId::from(hash)
    }

    /// Create a deterministic PublicKey from node name.
    fn create_executor_id(node_id: &NodeId) -> PublicKey {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(b"sim_executor:");
        hasher.update(node_id.as_str().as_bytes());
        let hash: [u8; 32] = hasher.finalize().into();
        PublicKey::from(hash)
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

    /// Get state digest (root hash of Merkle tree).
    pub fn state_digest(&self) -> StateDigest {
        StateDigest(self.storage.root_hash())
    }

    /// Get entity count (real entities only, excludes root and intermediate nodes).
    ///
    /// Uses `entity_metadata.len()` as the source of truth because:
    /// - Only "real" entities (inserted via `insert_entity*` methods) have metadata
    /// - Intermediate nodes created by `insert_entity_hierarchical` don't have metadata
    /// - This matches what production would count as entities
    pub fn entity_count(&self) -> usize {
        // Use actual storage leaf count (source of truth).
        // This counts only leaf nodes (actual entities), excluding intermediate nodes.
        // This ensures sync results are visible while hierarchical structures are counted correctly.
        self.storage.leaf_count()
    }

    /// Get root hash (for handshake).
    pub fn root_hash(&self) -> [u8; 32] {
        self.storage.root_hash()
    }

    /// Get context ID.
    pub fn context_id(&self) -> ContextId {
        self.storage.context_id()
    }

    /// Get reference to storage.
    pub fn storage(&self) -> &SimStorage {
        &self.storage
    }

    /// Get mutable reference to storage.
    pub fn storage_mut(&mut self) -> &mut SimStorage {
        &mut self.storage
    }

    /// Check if node has any state (real entities).
    pub fn has_any_state(&self) -> bool {
        self.has_state || !self.entity_metadata.is_empty()
    }

    /// Get tree statistics for debugging/testing.
    ///
    /// Returns (real_entities, total_tree_nodes, tree_depth).
    /// - `real_entities`: Count of actual entities (with metadata)
    /// - `total_tree_nodes`: All nodes in tree (includes root + intermediate)
    /// - `tree_depth`: Max depth from root to leaf
    pub fn tree_stats(&self) -> (usize, usize, u32) {
        let real_entities = self.entity_metadata.len();
        let total_tree_nodes = self.storage.entity_count();
        let tree_depth = self.storage.max_depth();
        (real_entities, total_tree_nodes, tree_depth)
    }

    // =========================================================================
    // Type Conversions (EntityId ↔ Id, EntityMetadata ↔ Metadata)
    // =========================================================================

    /// Convert simulation EntityId to storage Id.
    fn entity_id_to_storage_id(entity_id: EntityId) -> Id {
        Id::new(entity_id.0)
    }

    /// Convert simulation EntityMetadata to storage Metadata.
    ///
    /// Note: Storage Metadata doesn't have all simulation fields, so we store
    /// the full EntityMetadata separately in `entity_metadata` cache.
    fn entity_metadata_to_storage_metadata(_metadata: &EntityMetadata) -> Metadata {
        // Storage Metadata is simpler - use defaults for now
        // The simulation-specific fields are stored in entity_metadata cache
        Metadata::default()
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
    ///
    /// Delegates to `insert_entity_with_metadata` for Insert/Update to avoid
    /// duplicating the dual-write logic (storage + metadata cache).
    pub fn apply_storage_op(&mut self, op: StorageOp) {
        match op {
            StorageOp::Insert { id, data, metadata } | StorageOp::Update { id, data, metadata } => {
                // Delegate to avoid duplicating dual-write logic
                self.insert_entity_with_metadata(id, data, metadata);
            }
            StorageOp::Remove { id } => {
                let storage_id = Self::entity_id_to_storage_id(id);
                self.storage.remove_entity(storage_id);
                self.entity_metadata.remove(&id);
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
    ///
    /// # Edge case: zero capacity
    ///
    /// If capacity is 0, the incoming delta is immediately dropped (not added)
    /// and this method returns `false`. This matches production `DeltaBuffer::push`.
    ///
    /// # Performance (#5)
    ///
    /// Uses `VecDeque::pop_front()` for O(1) FIFO eviction instead of O(n) `Vec::remove(0)`.
    pub fn buffer_delta(&mut self, delta_id: DeltaId) -> bool {
        // Handle zero capacity: drop incoming delta immediately (matches production)
        if self.buffer_capacity == 0 {
            self.buffer_drops += 1;
            return false; // Dropped immediately
        }

        // Don't buffer duplicates (#2: deduplication prevents replay attacks)
        if self.delta_buffer.contains(&delta_id) {
            return true; // Already buffered, no action needed
        }

        // Check capacity - evict oldest if full
        if self.delta_buffer.len() >= self.buffer_capacity {
            // Evict oldest delta (front of buffer) - FIFO policy, O(1) with VecDeque
            if let Some(evicted_id) = self.delta_buffer.pop_front() {
                // Also remove its buffered operations
                self.buffered_operations.remove(&evicted_id);
                self.buffer_drops += 1;
            }
            self.delta_buffer.push_back(delta_id);
            false // Added with eviction
        } else {
            self.delta_buffer.push_back(delta_id);
            true // Added without eviction
        }
    }

    /// Clear delta buffer.
    pub fn clear_buffer(&mut self) {
        self.delta_buffer.clear();
    }

    /// Reset buffer state for a new sync session.
    ///
    /// Clears both delta_buffer and buffered_operations to match production
    /// behavior where `start_sync_session` creates a fresh `DeltaBuffer`.
    /// This ensures stale buffered deltas from a previous interrupted sync
    /// are discarded when a new sync session starts.
    pub fn reset_buffer_state(&mut self) {
        self.delta_buffer.clear();
        self.buffered_operations.clear();
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
        self.insert_entity_with_metadata(id, data, metadata);
    }

    /// Insert entity with full metadata.
    pub fn insert_entity_with_metadata(
        &mut self,
        id: EntityId,
        data: Vec<u8>,
        metadata: EntityMetadata,
    ) {
        let storage_id = Self::entity_id_to_storage_id(id);
        let storage_metadata = Self::entity_metadata_to_storage_metadata(&metadata);

        // Store in real Merkle tree
        self.storage.add_entity(storage_id, &data, storage_metadata);

        // Cache simulation metadata
        self.entity_metadata.insert(id, metadata);

        self.has_state = true;
    }

    /// Insert entity with hierarchical placement based on key prefix depth.
    ///
    /// This method creates a tree structure by:
    /// 1. Using the first `depth` bytes of the key to determine parent chain
    /// 2. Creating intermediate nodes as needed
    /// 3. Placing the entity at the correct depth level
    ///
    /// For example, with depth=3 and key [1,2,3,...]:
    /// - root -> node[1,0,0...] -> node[1,2,0...] -> entity[1,2,3,...]
    pub fn insert_entity_hierarchical(
        &mut self,
        id: EntityId,
        data: Vec<u8>,
        metadata: EntityMetadata,
        depth: u32,
    ) {
        let key = id.0;
        let depth = (depth as usize).min(MAX_HIERARCHICAL_DEPTH);

        // Ensure storage is initialized
        if self.storage.is_empty() {
            self.storage.init_root();
        }

        // Build parent chain based on key prefix
        let mut parent_id = self.storage.root_id();

        // Compute storage_id early to check for self-referencing cycles
        let storage_id = Self::entity_id_to_storage_id(id);

        for d in 1..=depth {
            // Create intermediate node ID from key prefix
            let mut intermediate_key = [0u8; 32];
            intermediate_key[..d].copy_from_slice(&key[..d]);

            let intermediate_id = Id::new(intermediate_key);

            // Skip if this intermediate ID equals the entity's storage ID
            // This prevents self-referencing cycles when key[depth..] are all zeros
            if intermediate_id == storage_id {
                continue;
            }

            // Create intermediate node if it doesn't exist
            if !self.storage.has_entity(intermediate_id) {
                self.storage.add_entity_with_parent(
                    intermediate_id,
                    parent_id,
                    &[], // Empty data for intermediate nodes
                    Metadata::default(),
                );
            }

            parent_id = intermediate_id;
        }

        // Add the actual entity under the deepest intermediate node
        let storage_metadata = Self::entity_metadata_to_storage_metadata(&metadata);
        self.storage
            .add_entity_with_parent(storage_id, parent_id, &data, storage_metadata);

        // Cache simulation metadata
        self.entity_metadata.insert(id, metadata);

        self.has_state = true;
    }

    /// Get entity by ID (real entities only).
    ///
    /// Returns a reconstructed `DigestEntity` combining data from storage
    /// and metadata from the cache. Returns `None` if:
    /// - Entity doesn't exist in storage, OR
    /// - Entity is an intermediate node (no metadata)
    pub fn get_entity(&self, id: &EntityId) -> Option<DigestEntity> {
        // Only return entities that have metadata (real entities, not intermediate nodes)
        let metadata = self.entity_metadata.get(id)?;

        let storage_id = Self::entity_id_to_storage_id(*id);

        // Get data from storage
        let data = self.storage.get_entity_data(storage_id)?;

        Some(DigestEntity {
            id: *id,
            data,
            metadata: metadata.clone(),
        })
    }

    /// Check if a real entity exists (excludes intermediate nodes).
    ///
    /// Returns true only for entities that have metadata (inserted via `insert_entity*`).
    pub fn has_entity(&self, id: &EntityId) -> bool {
        self.entity_metadata.contains_key(id)
    }

    /// Check if any node exists in storage at this ID (includes intermediate nodes).
    ///
    /// Use `has_entity()` for checking real entities.
    pub fn has_storage_node(&self, id: &EntityId) -> bool {
        let storage_id = Self::entity_id_to_storage_id(*id);
        self.storage.has_entity(storage_id)
    }

    /// Iterate over all real entities (excludes root and intermediate nodes).
    ///
    /// Only returns entities that have metadata (i.e., were inserted via `insert_entity*`).
    /// Intermediate nodes created by `insert_entity_hierarchical` are excluded.
    pub fn iter_entities(&self) -> impl Iterator<Item = DigestEntity> + '_ {
        // Use entity_metadata as source of truth for "real" entities
        self.entity_metadata
            .keys()
            .filter_map(move |id| self.get_entity(id))
    }

    /// Get all entity IDs (real entities only, excludes intermediate nodes).
    pub fn entity_ids(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.entity_metadata.keys().copied()
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

    // =========================================================================
    // Protocol Forcing (for testing)
    // =========================================================================

    /// Force a specific protocol for testing.
    ///
    /// When set, `select_protocol_for_sync()` returns this protocol instead of
    /// auto-selecting based on handshake parameters.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let mut alice = SimNode::new("alice");
    /// alice.force_protocol(SelectedProtocol::HashComparison);
    /// // Now any sync initiated by alice will use HashComparison
    /// ```
    pub fn force_protocol(&mut self, protocol: SelectedProtocol) {
        self.forced_protocol = Some(protocol);
    }

    /// Clear forced protocol, returning to auto-selection.
    pub fn clear_forced_protocol(&mut self) {
        self.forced_protocol = None;
    }

    /// Get the forced protocol, if any.
    pub fn forced_protocol(&self) -> Option<&SelectedProtocol> {
        self.forced_protocol.as_ref()
    }

    /// Select protocol for sync with another node.
    ///
    /// If a protocol has been forced via `force_protocol()`, returns that.
    /// Otherwise, uses the standard `select_protocol()` function.
    ///
    /// # Arguments
    ///
    /// * `remote` - The remote node's handshake
    ///
    /// # Returns
    ///
    /// The selected protocol and reason.
    pub fn select_protocol_for_sync(
        &mut self,
        remote: &SyncHandshake,
    ) -> (SelectedProtocol, String) {
        // Check for forced protocol first
        if let Some(protocol) = &self.forced_protocol {
            return (protocol.clone(), "forced for testing".to_string());
        }

        // Build our handshake and use standard selection
        let local = self.build_handshake();
        let selection = calimero_node_primitives::sync::protocol::select_protocol(&local, remote);

        // Convert SyncProtocol to SelectedProtocol
        let protocol = Self::sync_protocol_to_selected(&selection.protocol);
        (protocol, selection.reason.to_string())
    }

    /// Convert from primitives SyncProtocol to sim SelectedProtocol.
    pub fn sync_protocol_to_selected(protocol: &SyncProtocol) -> SelectedProtocol {
        match protocol {
            SyncProtocol::None => SelectedProtocol::None,
            SyncProtocol::Snapshot { compressed, .. } => SelectedProtocol::Snapshot {
                compressed: *compressed,
            },
            SyncProtocol::HashComparison { .. } => SelectedProtocol::HashComparison,
            SyncProtocol::DeltaSync { missing_delta_ids } => SelectedProtocol::DeltaSync {
                missing_count: missing_delta_ids.len(),
            },
            SyncProtocol::BloomFilter { filter_size, .. } => SelectedProtocol::BloomFilter {
                filter_size: *filter_size,
            },
            SyncProtocol::SubtreePrefetch { .. } => SelectedProtocol::SubtreePrefetch,
            SyncProtocol::LevelWise { max_depth } => SelectedProtocol::LevelWise {
                max_depth: *max_depth,
            },
        }
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
/// Now uses real Merkle tree storage via `SimStorage`.
impl LocalSyncState for SimNode {
    fn root_hash(&self) -> [u8; 32] {
        // Use real Merkle tree root hash from SimStorage
        self.storage.root_hash()
    }

    fn entity_count(&self) -> u64 {
        // Use actual storage count (excludes root)
        self.entity_count() as u64
    }

    /// Returns the maximum depth of the Merkle tree, excluding the root.
    ///
    /// # Semantics
    ///
    /// - `SimStorage::max_depth()` returns the raw tree depth (root-inclusive):
    ///   - Empty tree: 0
    ///   - Root only: 1
    ///   - Root + children: 2
    ///
    /// - `LocalSyncState::max_depth()` returns protocol-visible depth (root-exclusive):
    ///   - Empty tree: 0
    ///   - Root only (no entities): 0
    ///   - Entities at depth 1: 1
    ///
    /// This matches the protocol's expectation where depth indicates how many
    /// levels of entities exist below the root, which influences protocol
    /// selection (e.g., `SubtreePrefetch` for deep trees vs `Levelwise` for shallow).
    fn max_depth(&self) -> u32 {
        let depth = self.storage.max_depth();
        if depth > 0 {
            depth - 1
        } else {
            0
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

    /// Test: Zero capacity buffer drops all deltas immediately (#8 bug fix).
    ///
    /// Verifies that when buffer_capacity is 0:
    /// 1. Deltas are NOT added to the buffer
    /// 2. buffer_drops is incremented
    /// 3. buffer_delta returns false
    #[test]
    fn test_zero_capacity_drops_all() {
        let mut node = SimNode::with_buffer_capacity("zero_cap", 0);

        assert_eq!(node.buffer_size(), 0);
        assert_eq!(node.buffer_drops(), 0);

        // First delta should be dropped, not added
        let result = node.buffer_delta(DeltaId::from_bytes([1; 32]));
        assert!(!result, "Should return false when dropped");
        assert_eq!(node.buffer_size(), 0, "Buffer should remain empty");
        assert_eq!(node.buffer_drops(), 1, "Should increment drops");

        // Second delta also dropped
        let result = node.buffer_delta(DeltaId::from_bytes([2; 32]));
        assert!(!result);
        assert_eq!(node.buffer_size(), 0);
        assert_eq!(node.buffer_drops(), 2);

        // Third delta also dropped
        let result = node.buffer_delta(DeltaId::from_bytes([3; 32]));
        assert!(!result);
        assert_eq!(node.buffer_size(), 0);
        assert_eq!(node.buffer_drops(), 3);
    }

    /// Test: Buffer eviction with VecDeque is O(1) and preserves FIFO order.
    #[test]
    fn test_buffer_fifo_eviction_order() {
        let mut node = SimNode::with_buffer_capacity("evict_test", 3);

        // Fill buffer
        node.buffer_delta(DeltaId::from_bytes([1; 32]));
        node.buffer_delta(DeltaId::from_bytes([2; 32]));
        node.buffer_delta(DeltaId::from_bytes([3; 32]));
        assert_eq!(node.buffer_size(), 3);
        assert_eq!(node.buffer_drops(), 0);

        // Add 4th - should evict 1st
        let result = node.buffer_delta(DeltaId::from_bytes([4; 32]));
        assert!(!result, "Should return false when eviction occurred");
        assert_eq!(node.buffer_size(), 3);
        assert_eq!(node.buffer_drops(), 1);

        // Add 5th - should evict 2nd
        node.buffer_delta(DeltaId::from_bytes([5; 32]));
        assert_eq!(node.buffer_size(), 3);
        assert_eq!(node.buffer_drops(), 2);

        // Verify buffer contains [3, 4, 5] in FIFO order
        assert!(node.delta_buffer.contains(&DeltaId::from_bytes([3; 32])));
        assert!(node.delta_buffer.contains(&DeltaId::from_bytes([4; 32])));
        assert!(node.delta_buffer.contains(&DeltaId::from_bytes([5; 32])));
        assert!(!node.delta_buffer.contains(&DeltaId::from_bytes([1; 32])));
        assert!(!node.delta_buffer.contains(&DeltaId::from_bytes([2; 32])));
    }

    #[test]
    fn test_hierarchical_insertion_excludes_intermediate_nodes() {
        let mut node = SimNode::new("alice");

        // Insert 3 entities hierarchically with depth=3
        // This creates intermediate nodes at depth 1, 2, 3 for each unique prefix
        let key1 = EntityId::from_bytes([
            1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let key2 = EntityId::from_bytes([
            1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let key3 = EntityId::from_bytes([
            2, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 0,
            0, 0, 0,
        ]);

        let metadata = EntityMetadata::new(CrdtType::LwwRegister, 0);

        node.insert_entity_hierarchical(key1, vec![1], metadata.clone(), 3);
        node.insert_entity_hierarchical(key2, vec![2], metadata.clone(), 3);
        node.insert_entity_hierarchical(key3, vec![3], metadata.clone(), 3);

        // Should count only 3 real entities, not intermediate nodes
        assert_eq!(node.entity_count(), 3, "Should count only real entities");

        // Verify tree has more nodes (root + intermediate + real entities)
        let (real, total, depth) = node.tree_stats();
        assert_eq!(real, 3, "Should have 3 real entities");
        assert!(
            total > 3,
            "Tree should have intermediate nodes: got {}",
            total
        );
        assert!(depth > 1, "Tree should have depth > 1: got {}", depth);

        // iter_entities should only return the 3 real entities
        let entities: Vec<_> = node.iter_entities().collect();
        assert_eq!(
            entities.len(),
            3,
            "iter_entities should return only real entities"
        );

        // has_entity should only return true for real entities
        assert!(node.has_entity(&key1));
        assert!(node.has_entity(&key2));
        assert!(node.has_entity(&key3));

        // Intermediate node IDs should NOT be returned by has_entity
        let intermediate = EntityId::from_bytes([
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        assert!(
            !node.has_entity(&intermediate),
            "Intermediate nodes should not be 'real' entities"
        );

        // But has_storage_node should return true for intermediate nodes
        assert!(
            node.has_storage_node(&intermediate),
            "Intermediate nodes should exist in storage"
        );
    }

    #[test]
    fn test_hierarchical_insertion_reuses_intermediate_nodes() {
        let mut node = SimNode::new("alice");

        // Insert two entities that share the same prefix [1,1,...]
        let key1 = EntityId::from_bytes([
            1, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 0, 0,
        ]);
        let key2 = EntityId::from_bytes([
            1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0,
            0, 0, 0,
        ]);

        let metadata = EntityMetadata::new(CrdtType::LwwRegister, 0);

        node.insert_entity_hierarchical(key1, vec![1], metadata.clone(), 3);
        let (_, total_after_first, _) = node.tree_stats();

        node.insert_entity_hierarchical(key2, vec![2], metadata.clone(), 3);
        let (real, total_after_second, _) = node.tree_stats();

        // Second insertion should reuse intermediate nodes [1,0,0...] and [1,1,0...]
        // So total should only increase by: 1 new intermediate at depth 3 + 1 real entity = 2
        assert_eq!(real, 2);
        assert!(
            total_after_second < total_after_first + 4,
            "Should reuse intermediate nodes: first={}, second={}",
            total_after_first,
            total_after_second
        );
    }
}
