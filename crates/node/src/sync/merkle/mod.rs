//! Merkle tree construction and synchronization for efficient partial sync.
//!
//! This module implements Phase 2 of the sync protocol: Merkle tree-based
//! differential sync. It allows nodes with partial state overlap to efficiently
//! identify and transfer only the differing chunks.
//!
//! ## Hashing Rules
//!
//! ```text
//! leaf_hash = H("leaf" || index || payload_hash || uncompressed_len || start_key || end_key)
//! node_hash = H("node" || level || child_hashes...)
//! ```
//!
//! ## Module Structure
//!
//! - [`tree`]: Merkle tree construction and node hashing
//! - [`traversal`]: Pure state machine for tree traversal during sync
//! - [`validation`]: Request validation and range helpers
//! - [`handlers`]: `SyncManager` request/response handling

mod handlers;
mod traversal;
mod tree;
mod validation;

// Re-export types used by other modules in the sync crate
pub use tree::MerkleTree;
pub use validation::{parse_boundary_for_merkle, BoundaryParseResult, MerkleSyncResult};
