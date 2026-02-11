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
//! └── snapshot.rs        # SnapshotPage, BroadcastMessage, StreamMessage, etc.
//! ```

#![expect(single_use_lifetimes, reason = "borsh shenanigans")]

// =============================================================================
// Submodules
// =============================================================================

pub mod bloom_filter;
pub mod delta;
pub mod handshake;
pub mod hash_comparison;
pub mod protocol;
pub mod snapshot;

// =============================================================================
// Re-exports
// =============================================================================

// Handshake types
pub use handshake::{
    SyncCapabilities, SyncHandshake, SyncHandshakeResponse, SYNC_PROTOCOL_VERSION,
};

// Protocol types and selection
pub use protocol::{
    calculate_divergence, is_protocol_supported, select_protocol, select_protocol_with_fallback,
    ProtocolSelection, SyncProtocol, SyncProtocolKind,
};

// Delta sync types
pub use delta::{
    DeltaApplyResult, DeltaPayload, DeltaSyncRequest, DeltaSyncResponse,
    DEFAULT_DELTA_SYNC_THRESHOLD,
};

// Hash comparison types
pub use hash_comparison::{
    compare_tree_nodes, CrdtType, LeafMetadata, TreeCompareResult, TreeLeafData, TreeNode,
    TreeNodeRequest, TreeNodeResponse, MAX_CHILDREN_PER_NODE, MAX_LEAF_VALUE_SIZE,
    MAX_NODES_PER_RESPONSE, MAX_TREE_DEPTH,
};

// Bloom filter types
pub use bloom_filter::{
    BloomFilterRequest, BloomFilterResponse, DeltaIdBloomFilter, DEFAULT_BLOOM_FP_RATE,
};

// Snapshot and wire protocol types
pub use snapshot::{
    check_snapshot_safety, BroadcastMessage, InitPayload, MessagePayload, SnapshotBoundaryRequest,
    SnapshotBoundaryResponse, SnapshotComplete, SnapshotCursor, SnapshotEntity, SnapshotEntityPage,
    SnapshotError, SnapshotPage, SnapshotRequest, SnapshotStreamRequest, SnapshotVerifyResult,
    StreamMessage, DEFAULT_SNAPSHOT_PAGE_SIZE, MAX_DAG_HEADS, MAX_ENTITIES_PER_PAGE,
    MAX_ENTITY_DATA_SIZE, MAX_SNAPSHOT_PAGES, MAX_SNAPSHOT_PAGE_SIZE,
};
