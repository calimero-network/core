//! Sync protocol types for state synchronization between nodes.
//!
//! This module provides types for the various sync protocols:
//!
//! - **Handshake**: Initial negotiation between peers ([`handshake`])
//! - **Protocol Selection**: Choosing the optimal sync strategy ([`protocol`])
//! - **Delta Sync**: DAG-based delta synchronization ([`delta`])
//! - **HashComparison**: Merkle tree traversal sync ([`hash_comparison`])
//! - **BloomFilter**: Bloom filter-based sync for large trees ([`bloom_filter`])
//! - **Snapshot**: Full state transfer for fresh nodes ([`snapshot`])
//! - **SubtreePrefetch**: Subtree prefetch for deep trees with clustered changes ([`subtree`])
//! - **LevelWise**: Level-by-level sync for wide shallow trees ([`levelwise`])
//!
//! # Module Organization
//!
//! Each sync protocol has its own module with types and tests:
//!
//! ```text
//! sync/
//! ├── handshake.rs       # SyncHandshake, SyncCapabilities, etc.
//! ├── protocol.rs        # SyncProtocol, SyncProtocolKind, select_protocol()
//! ├── delta.rs           # DeltaSyncRequest, DeltaPayload, etc.
//! ├── hash_comparison.rs # TreeNode, TreeNodeRequest, compare_tree_nodes()
//! ├── bloom_filter.rs    # DeltaIdBloomFilter, BloomFilterRequest, etc.
//! ├── snapshot.rs        # SnapshotPage, BroadcastMessage, StreamMessage, etc.
//! ├── subtree.rs         # SubtreePrefetchRequest, SubtreeData, etc.
//! └── levelwise.rs       # LevelWiseRequest, LevelWiseResponse, etc.
//! ```

#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

// =============================================================================
// Submodules
// =============================================================================

pub mod bloom_filter;
pub mod delta;
pub mod handshake;
pub mod hash_comparison;
pub mod levelwise;
pub mod protocol;
pub mod protocol_trait;
pub mod snapshot;
pub mod state_machine;
pub mod storage_bridge;
pub mod subtree;
pub mod transport;
pub mod wire;

// =============================================================================
// Re-exports
// =============================================================================

// Handshake types
pub use handshake::{
    SYNC_PROTOCOL_VERSION, SyncCapabilities, SyncHandshake, SyncHandshakeResponse,
};

// Protocol types and selection
pub use protocol::{
    ProtocolSelection, SyncProtocol, SyncProtocolKind, calculate_divergence, is_protocol_supported,
    select_protocol, select_protocol_with_fallback,
};

// Delta sync types
pub use delta::{
    DEFAULT_DELTA_SYNC_THRESHOLD, DeltaApplyResult, DeltaPayload, DeltaSyncRequest,
    DeltaSyncResponse,
};

// Hash comparison types
pub use hash_comparison::{
    CrdtType, LeafMetadata, MAX_CHILDREN_PER_NODE, MAX_LEAF_VALUE_SIZE, MAX_NODES_PER_RESPONSE,
    MAX_TREE_DEPTH, TreeCompareResult, TreeLeafData, TreeNode, TreeNodeRequest, TreeNodeResponse,
    compare_tree_nodes,
};

// Bloom filter types
pub use bloom_filter::{
    BloomFilterRequest, BloomFilterResponse, DEFAULT_BLOOM_FP_RATE, DeltaIdBloomFilter,
};

// Wire protocol types (used by all sync protocols)
pub use wire::{InitPayload, MAX_TREE_REQUEST_DEPTH, MessagePayload, StreamMessage};

// Snapshot types
pub use snapshot::{
    BroadcastMessage, DEFAULT_SNAPSHOT_PAGE_SIZE, MAX_COMPRESSED_PAYLOAD_SIZE, MAX_DAG_HEADS,
    MAX_ENTITIES_PER_PAGE, MAX_ENTITY_DATA_SIZE, MAX_SIGNED_GROUP_OP_PAYLOAD_BYTES,
    MAX_SNAPSHOT_PAGE_SIZE, MAX_SNAPSHOT_PAGES, SnapshotBoundaryRequest, SnapshotBoundaryResponse,
    SnapshotComplete, SnapshotCursor, SnapshotEntity, SnapshotEntityPage, SnapshotError,
    SnapshotPage, SnapshotRequest, SnapshotStreamRequest, SnapshotVerifyResult,
    check_snapshot_safety,
};

// Subtree prefetch types
pub use subtree::{
    DEEP_TREE_THRESHOLD, DEFAULT_SUBTREE_MAX_DEPTH, MAX_CLUSTERED_SUBTREES, MAX_DIVERGENCE_RATIO,
    MAX_ENTITIES_PER_SUBTREE, MAX_SUBTREE_DEPTH, MAX_SUBTREES_PER_REQUEST, MAX_TOTAL_ENTITIES,
    SubtreeData, SubtreePrefetchRequest, SubtreePrefetchResponse, should_use_subtree_prefetch,
};

// LevelWise sync types
pub use levelwise::{
    LevelCompareResult, LevelNode, LevelWiseRequest, LevelWiseResponse, MAX_LEVELWISE_DEPTH,
    MAX_NODES_PER_LEVEL, MAX_PARENTS_PER_REQUEST, MAX_REQUESTS_PER_SESSION, compare_level_nodes,
    should_use_levelwise,
};

// State machine types (shared between SyncManager and SimNode)
pub use state_machine::{
    LocalSyncState, build_handshake, build_handshake_from_raw, estimate_entity_count,
    estimate_max_depth,
};

// Transport abstraction (for production streams and simulation)
pub use transport::{EncryptionState, SyncTransport};

// Protocol trait (common interface for all sync protocols)
pub use protocol_trait::SyncProtocolExecutor;

// Storage bridge (RuntimeEnv creation for sync protocols)
pub use storage_bridge::create_runtime_env;
