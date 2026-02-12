//! Sync protocol state machine (CIP §3 - Side-Effect Model).
//!
//! Provides a pure, deterministic protocol logic layer that can be shared
//! between production code (`SyncManager`) and simulation (`SimNode`).
//!
//! # Design
//!
//! The state machine follows the effects-only model from the simulation spec:
//! - Protocol logic is pure (no I/O, no async)
//! - Returns `SyncActions` effects that the runtime applies
//! - Same code works in production and simulation
//!
//! # Usage
//!
//! ```rust,ignore
//! // Any type can provide its local state
//! impl LocalSyncState for MyNode {
//!     fn root_hash(&self) -> [u8; 32] { ... }
//!     fn entity_count(&self) -> u64 { ... }
//!     fn max_depth(&self) -> u32 { ... }
//!     fn dag_heads(&self) -> Vec<[u8; 32]> { ... }
//! }
//!
//! // Build handshake using shared logic
//! let handshake = build_handshake(&my_node);
//!
//! // Use select_protocol() for negotiation
//! let selection = select_protocol(&local_hs, &remote_hs);
//! ```

use super::handshake::SyncHandshake;

// =============================================================================
// Local State Trait
// =============================================================================

/// Trait for accessing local sync state.
///
/// Implement this trait to enable building handshakes and protocol selection.
/// Both `SyncManager` (production) and `SimNode` (simulation) implement this,
/// ensuring consistent behavior across both environments.
pub trait LocalSyncState {
    /// Current Merkle root hash.
    ///
    /// Returns `[0; 32]` for fresh nodes with no state.
    fn root_hash(&self) -> [u8; 32];

    /// Number of entities in local storage.
    ///
    /// Used for divergence calculation in protocol selection.
    fn entity_count(&self) -> u64;

    /// Maximum depth of the Merkle tree.
    ///
    /// Used to select optimal sync protocol:
    /// - Depth > 3 with low divergence → SubtreePrefetch
    /// - Depth 1-2 with many children → LevelWise
    fn max_depth(&self) -> u32;

    /// Current DAG heads (latest delta IDs).
    ///
    /// Used for delta sync compatibility checking.
    fn dag_heads(&self) -> Vec<[u8; 32]>;

    /// Whether this node has any state.
    ///
    /// Default implementation: `root_hash != [0; 32]`
    fn has_state(&self) -> bool {
        self.root_hash() != [0; 32]
    }
}

// =============================================================================
// Handshake Builder
// =============================================================================

/// Build a `SyncHandshake` from any type implementing `LocalSyncState`.
///
/// This is the canonical way to create handshakes, ensuring consistent
/// behavior between production and simulation code.
///
/// # Example
///
/// ```rust,ignore
/// let handshake = build_handshake(&my_node);
/// ```
#[must_use]
pub fn build_handshake<T: LocalSyncState>(state: &T) -> SyncHandshake {
    SyncHandshake::new(
        state.root_hash(),
        state.entity_count(),
        state.max_depth(),
        state.dag_heads(),
    )
}

/// Build a `SyncHandshake` from raw state values.
///
/// Useful when you have the values but don't have a `LocalSyncState` implementor.
#[must_use]
pub fn build_handshake_from_raw(
    root_hash: [u8; 32],
    entity_count: u64,
    max_depth: u32,
    dag_heads: Vec<[u8; 32]>,
) -> SyncHandshake {
    SyncHandshake::new(root_hash, entity_count, max_depth, dag_heads)
}

// =============================================================================
// Estimation Helpers
// =============================================================================

/// Estimate entity count from available data.
///
/// This provides a consistent heuristic used when exact entity count is unknown.
/// The estimation is based on:
/// - Zero if root hash is zero (fresh node)
/// - dag_heads.len() if available, minimum 1 if has state
///
/// # Arguments
///
/// * `root_hash` - The Merkle root hash
/// * `dag_heads_len` - Number of DAG heads
#[must_use]
pub fn estimate_entity_count(root_hash: [u8; 32], dag_heads_len: usize) -> u64 {
    if root_hash == [0; 32] {
        0
    } else if dag_heads_len == 0 {
        1 // Has state but no heads - assume at least 1 entity
    } else {
        dag_heads_len as u64
    }
}

/// Estimate max depth from entity count.
///
/// Uses log2 approximation for balanced tree estimation:
/// `max_depth ≈ ceil(log2(entity_count))`
///
/// This is bounded to a reasonable maximum to prevent overflow.
///
/// # Arguments
///
/// * `entity_count` - Number of entities in the tree
#[must_use]
pub fn estimate_max_depth(entity_count: u64) -> u32 {
    if entity_count == 0 {
        0
    } else {
        // log2(n) ≈ 64 - leading_zeros(n)
        // For a balanced tree, depth is roughly log2(entity_count)
        let log2_approx = 64u32.saturating_sub(entity_count.leading_zeros());
        log2_approx.max(1).min(32)
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Test implementation of LocalSyncState
    struct TestNode {
        root_hash: [u8; 32],
        entity_count: u64,
        max_depth: u32,
        dag_heads: Vec<[u8; 32]>,
    }

    impl LocalSyncState for TestNode {
        fn root_hash(&self) -> [u8; 32] {
            self.root_hash
        }

        fn entity_count(&self) -> u64 {
            self.entity_count
        }

        fn max_depth(&self) -> u32 {
            self.max_depth
        }

        fn dag_heads(&self) -> Vec<[u8; 32]> {
            self.dag_heads.clone()
        }
    }

    #[test]
    fn test_build_handshake_fresh_node() {
        let node = TestNode {
            root_hash: [0; 32],
            entity_count: 0,
            max_depth: 0,
            dag_heads: vec![],
        };

        let hs = build_handshake(&node);

        assert_eq!(hs.root_hash, [0; 32]);
        assert_eq!(hs.entity_count, 0);
        assert_eq!(hs.max_depth, 0);
        assert!(hs.dag_heads.is_empty());
        assert!(!hs.has_state);
    }

    #[test]
    fn test_build_handshake_initialized_node() {
        let node = TestNode {
            root_hash: [42; 32],
            entity_count: 100,
            max_depth: 5,
            dag_heads: vec![[1; 32], [2; 32]],
        };

        let hs = build_handshake(&node);

        assert_eq!(hs.root_hash, [42; 32]);
        assert_eq!(hs.entity_count, 100);
        assert_eq!(hs.max_depth, 5);
        assert_eq!(hs.dag_heads.len(), 2);
        assert!(hs.has_state);
    }

    #[test]
    fn test_estimate_entity_count() {
        // Fresh node
        assert_eq!(estimate_entity_count([0; 32], 0), 0);

        // Has state, no heads
        assert_eq!(estimate_entity_count([1; 32], 0), 1);

        // Has state with heads
        assert_eq!(estimate_entity_count([1; 32], 5), 5);
    }

    #[test]
    fn test_estimate_max_depth() {
        assert_eq!(estimate_max_depth(0), 0);
        assert_eq!(estimate_max_depth(1), 1);
        assert_eq!(estimate_max_depth(2), 2);
        assert_eq!(estimate_max_depth(16), 5); // log2(16) = 4, +1 = 5
        assert_eq!(estimate_max_depth(256), 9); // log2(256) = 8, +1 = 9
    }

    #[test]
    fn test_has_state_default_implementation() {
        let fresh = TestNode {
            root_hash: [0; 32],
            entity_count: 0,
            max_depth: 0,
            dag_heads: vec![],
        };
        assert!(!fresh.has_state());

        let initialized = TestNode {
            root_hash: [1; 32],
            entity_count: 1,
            max_depth: 1,
            dag_heads: vec![],
        };
        assert!(initialized.has_state());
    }
}
