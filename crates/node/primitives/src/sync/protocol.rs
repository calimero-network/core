//! Sync protocol types and selection logic (CIP §2.3 - Protocol Selection Rules).
//!
//! Defines the available sync protocols and the logic to select the optimal one.

use borsh::{BorshDeserialize, BorshSerialize};

use super::handshake::{SyncCapabilities, SyncHandshake};
use super::levelwise::should_use_levelwise;

// =============================================================================
// Protocol Kind (Discriminant-only)
// =============================================================================

/// Protocol capability identifier for sync negotiation.
///
/// This is a discriminant-only enum used for advertising which sync protocols
/// a node supports. Unlike [`SyncProtocol`], this does not carry protocol-specific
/// data, making it suitable for capability comparison with `contains()` and equality.
///
/// See CIP §2 - Sync Handshake Protocol.
///
/// **IMPORTANT**: Keep variants in sync with [`SyncProtocol`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, BorshSerialize, BorshDeserialize)]
pub enum SyncProtocolKind {
    /// No sync needed - root hashes already match.
    None,
    /// Delta-based sync via DAG traversal.
    DeltaSync,
    /// Hash-based Merkle tree comparison.
    HashComparison,
    /// Full state snapshot transfer.
    Snapshot,
    /// Bloom filter-based quick diff.
    BloomFilter,
    /// Subtree prefetch for deep localized changes.
    SubtreePrefetch,
    /// Level-wise sync for wide shallow trees.
    LevelWise,
}

// =============================================================================
// Protocol (With Data)
// =============================================================================

/// Sync protocol selection for negotiation.
///
/// Each variant represents a different synchronization strategy with different
/// trade-offs in terms of bandwidth, latency, and computational overhead.
///
/// See CIP §1 - Sync Protocol Types.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub enum SyncProtocol {
    /// No sync needed - root hashes already match.
    None,

    /// Delta-based sync via DAG traversal.
    ///
    /// Best for: Small gaps, real-time updates.
    DeltaSync {
        /// Delta IDs that the requester is missing.
        missing_delta_ids: Vec<[u8; 32]>,
    },

    /// Hash-based Merkle tree comparison.
    ///
    /// Best for: General-purpose catch-up, 10-50% divergence.
    HashComparison {
        /// Root hash to compare against.
        root_hash: [u8; 32],
        /// Subtree roots that differ (if known).
        divergent_subtrees: Vec<[u8; 32]>,
    },

    /// Full state snapshot transfer.
    ///
    /// **CRITICAL**: Only valid for fresh nodes (Invariant I5).
    /// Initialized nodes MUST use state-based sync with CRDT merge instead.
    Snapshot {
        /// Whether the snapshot is compressed.
        compressed: bool,
        /// Whether the responder guarantees snapshot is verifiable.
        verified: bool,
    },

    /// Bloom filter-based quick diff.
    ///
    /// Best for: Large trees with small diff (<10% divergence).
    BloomFilter {
        /// Size of the bloom filter in bits.
        filter_size: u64,
        /// Expected false positive rate (0.0 to 1.0).
        ///
        /// **Note**: Validation of bounds is performed when constructing the actual
        /// bloom filter, not at protocol negotiation time. Invalid values will cause
        /// filter construction to fail gracefully.
        false_positive_rate: f64,
    },

    /// Subtree prefetch for deep localized changes.
    ///
    /// Best for: Deep hierarchies with localized changes.
    SubtreePrefetch {
        /// Root IDs of subtrees to prefetch.
        subtree_roots: Vec<[u8; 32]>,
    },

    /// Level-wise sync for wide shallow trees.
    ///
    /// Best for: Trees with depth ≤ 2 and many children.
    LevelWise {
        /// Maximum depth to sync.
        max_depth: u32,
    },
}

impl Default for SyncProtocol {
    fn default() -> Self {
        Self::None
    }
}

impl SyncProtocol {
    /// Returns the protocol kind (discriminant) for this protocol.
    ///
    /// Useful for capability matching where the protocol-specific data is irrelevant.
    #[must_use]
    pub fn kind(&self) -> SyncProtocolKind {
        SyncProtocolKind::from(self)
    }
}

impl From<&SyncProtocol> for SyncProtocolKind {
    fn from(protocol: &SyncProtocol) -> Self {
        match protocol {
            SyncProtocol::None => Self::None,
            SyncProtocol::DeltaSync { .. } => Self::DeltaSync,
            SyncProtocol::HashComparison { .. } => Self::HashComparison,
            SyncProtocol::Snapshot { .. } => Self::Snapshot,
            SyncProtocol::BloomFilter { .. } => Self::BloomFilter,
            SyncProtocol::SubtreePrefetch { .. } => Self::SubtreePrefetch,
            SyncProtocol::LevelWise { .. } => Self::LevelWise,
        }
    }
}

// =============================================================================
// Protocol Selection
// =============================================================================

/// Result of protocol selection with reasoning.
#[derive(Clone, Debug)]
pub struct ProtocolSelection {
    /// The selected protocol.
    pub protocol: SyncProtocol,
    /// Human-readable reason for the selection (for logging).
    pub reason: &'static str,
}

/// Calculate the divergence ratio between two handshakes.
///
/// Formula: `|local.entity_count - remote.entity_count| / max(remote.entity_count, 1)`
///
/// Returns a value in [0.0, ∞), where:
/// - 0.0 = identical entity counts
/// - 1.0 = 100% divergence (e.g., local=0, remote=100)
/// - >1.0 = local has more entities than remote
#[must_use]
pub fn calculate_divergence(local: &SyncHandshake, remote: &SyncHandshake) -> f64 {
    // Use abs_diff to avoid overflow when entity_count exceeds i64::MAX
    let diff = local.entity_count.abs_diff(remote.entity_count);
    let denominator = remote.entity_count.max(1);
    diff as f64 / denominator as f64
}

/// Select the optimal sync protocol based on handshake information.
///
/// Implements the decision table from CIP §2.3:
///
/// | # | Condition | Selected Protocol |
/// |---|-----------|-------------------|
/// | 1 | `root_hash` match | `None` |
/// | 2 | `!has_state` (fresh node) | `Snapshot` |
/// | 3 | `has_state` AND divergence >50% | `HashComparison` |
/// | 4 | `max_depth` >3 AND divergence <20% | `SubtreePrefetch` |
/// | 5 | `entity_count` >50 AND divergence <10% | `BloomFilter` |
/// | 6 | `max_depth` 1-2 AND avg children/level >10 | `LevelWise` |
/// | 7 | (default) | `HashComparison` |
///
/// **CRITICAL (Invariant I5)**: Snapshot is NEVER selected for initialized nodes.
/// This prevents silent data loss from overwriting local CRDT state.
#[must_use]
pub fn select_protocol(local: &SyncHandshake, remote: &SyncHandshake) -> ProtocolSelection {
    // Rule 1: Already synced - no action needed
    if local.root_hash == remote.root_hash {
        return ProtocolSelection {
            protocol: SyncProtocol::None,
            reason: "root hashes match, already in sync",
        };
    }

    // Check version compatibility first
    if !local.is_version_compatible(remote) {
        // Version mismatch - fall back to HashComparison as safest option
        return ProtocolSelection {
            protocol: SyncProtocol::HashComparison {
                root_hash: remote.root_hash,
                divergent_subtrees: vec![],
            },
            reason: "version mismatch, using safe fallback",
        };
    }

    // Rule 2: Fresh node - Snapshot allowed
    // CRITICAL: This is the ONLY case where Snapshot is permitted
    if !local.has_state {
        return ProtocolSelection {
            protocol: SyncProtocol::Snapshot {
                compressed: remote.entity_count > 100,
                verified: true,
            },
            reason: "fresh node bootstrap via snapshot",
        };
    }

    // From here on, local HAS state - NEVER use Snapshot (Invariant I5)
    let divergence = calculate_divergence(local, remote);

    // Rule 3: Large divergence (>50%) - use HashComparison with CRDT merge
    if divergence > 0.5 {
        return ProtocolSelection {
            protocol: SyncProtocol::HashComparison {
                root_hash: remote.root_hash,
                divergent_subtrees: vec![],
            },
            reason: "high divergence (>50%), using hash comparison with CRDT merge",
        };
    }

    // Rule 4: Deep tree with localized changes
    if remote.max_depth > 3 && divergence < 0.2 {
        return ProtocolSelection {
            protocol: SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![], // Will be populated during sync
            },
            reason: "deep tree with low divergence, using subtree prefetch",
        };
    }

    // Rule 5: Large tree with small diff
    if remote.entity_count > 50 && divergence < 0.1 {
        return ProtocolSelection {
            protocol: SyncProtocol::BloomFilter {
                // ~10 bits per entity, max 10k; saturating to avoid overflow on huge counts
                filter_size: remote.entity_count.saturating_mul(10).min(10_000),
                false_positive_rate: 0.01,
            },
            reason: "large tree with small divergence, using bloom filter",
        };
    }

    // Rule 6: Wide shallow tree (depth 1-2 with many children per level)
    // Delegate to the canonical heuristic in levelwise module
    let max_depth_usize = remote.max_depth as usize;
    let avg_children_per_level = if remote.max_depth > 0 {
        (remote.entity_count / u64::from(remote.max_depth)) as usize
    } else {
        0
    };
    if should_use_levelwise(max_depth_usize, avg_children_per_level) {
        return ProtocolSelection {
            protocol: SyncProtocol::LevelWise {
                max_depth: remote.max_depth,
            },
            reason: "wide shallow tree, using level-wise sync",
        };
    }

    // Rule 7: Default fallback
    ProtocolSelection {
        protocol: SyncProtocol::HashComparison {
            root_hash: remote.root_hash,
            divergent_subtrees: vec![],
        },
        reason: "default: using hash comparison",
    }
}

/// Check if a protocol kind is supported by the remote peer.
#[must_use]
pub fn is_protocol_supported(protocol: &SyncProtocol, capabilities: &SyncCapabilities) -> bool {
    capabilities.supported_protocols.contains(&protocol.kind())
}

/// Select protocol with fallback if preferred is not supported.
///
/// Tries the preferred protocol first, then falls back through the decision
/// table until a mutually supported protocol is found.
#[must_use]
pub fn select_protocol_with_fallback(
    local: &SyncHandshake,
    remote: &SyncHandshake,
    remote_capabilities: &SyncCapabilities,
) -> ProtocolSelection {
    let preferred = select_protocol(local, remote);

    // Check if preferred protocol is supported
    if is_protocol_supported(&preferred.protocol, remote_capabilities) {
        return preferred;
    }

    // Fallback: HashComparison is always safe for initialized nodes
    if local.has_state {
        let fallback = SyncProtocol::HashComparison {
            root_hash: remote.root_hash,
            divergent_subtrees: vec![],
        };
        if is_protocol_supported(&fallback, remote_capabilities) {
            return ProtocolSelection {
                protocol: fallback,
                reason: "fallback to hash comparison (preferred not supported)",
            };
        }
    }

    // Last resort: None (will need manual intervention)
    ProtocolSelection {
        protocol: SyncProtocol::None,
        reason: "no mutually supported protocol found",
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::handshake::SYNC_PROTOCOL_VERSION;

    #[test]
    fn test_sync_protocol_roundtrip() {
        let protocols = vec![
            SyncProtocol::None,
            SyncProtocol::DeltaSync {
                missing_delta_ids: vec![[1; 32], [2; 32]],
            },
            SyncProtocol::HashComparison {
                root_hash: [3; 32],
                divergent_subtrees: vec![[4; 32]],
            },
            SyncProtocol::Snapshot {
                compressed: true,
                verified: false,
            },
            SyncProtocol::BloomFilter {
                filter_size: 1024,
                false_positive_rate: 0.01,
            },
            SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![[5; 32], [6; 32]],
            },
            SyncProtocol::LevelWise { max_depth: 3 },
        ];

        for protocol in protocols {
            let encoded = borsh::to_vec(&protocol).expect("serialize");
            let decoded: SyncProtocol = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(protocol, decoded);
        }
    }

    #[test]
    fn test_sync_protocol_kind_roundtrip() {
        let kinds = vec![
            SyncProtocolKind::None,
            SyncProtocolKind::DeltaSync,
            SyncProtocolKind::HashComparison,
            SyncProtocolKind::Snapshot,
            SyncProtocolKind::BloomFilter,
            SyncProtocolKind::SubtreePrefetch,
            SyncProtocolKind::LevelWise,
        ];

        for kind in kinds {
            let encoded = borsh::to_vec(&kind).expect("serialize");
            let decoded: SyncProtocolKind = borsh::from_slice(&encoded).expect("deserialize");
            assert_eq!(kind, decoded);
        }
    }

    #[test]
    fn test_sync_protocol_kind_conversion() {
        // Test kind() method and From trait
        assert_eq!(SyncProtocol::None.kind(), SyncProtocolKind::None);
        assert_eq!(
            SyncProtocol::DeltaSync {
                missing_delta_ids: vec![[1; 32]]
            }
            .kind(),
            SyncProtocolKind::DeltaSync
        );
        assert_eq!(
            SyncProtocol::HashComparison {
                root_hash: [2; 32],
                divergent_subtrees: vec![]
            }
            .kind(),
            SyncProtocolKind::HashComparison
        );
        assert_eq!(
            SyncProtocol::Snapshot {
                compressed: true,
                verified: true
            }
            .kind(),
            SyncProtocolKind::Snapshot
        );
        assert_eq!(
            SyncProtocol::BloomFilter {
                filter_size: 1024,
                false_positive_rate: 0.01
            }
            .kind(),
            SyncProtocolKind::BloomFilter
        );
        assert_eq!(
            SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![]
            }
            .kind(),
            SyncProtocolKind::SubtreePrefetch
        );
        assert_eq!(
            SyncProtocol::LevelWise { max_depth: 5 }.kind(),
            SyncProtocolKind::LevelWise
        );

        // Test From trait directly
        let protocol = SyncProtocol::HashComparison {
            root_hash: [3; 32],
            divergent_subtrees: vec![],
        };
        let kind: SyncProtocolKind = (&protocol).into();
        assert_eq!(kind, SyncProtocolKind::HashComparison);
    }

    #[test]
    fn test_calculate_divergence() {
        // Same counts
        let local = SyncHandshake::new([1; 32], 100, 5, vec![]);
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        assert!((calculate_divergence(&local, &remote) - 0.0).abs() < f64::EPSILON);

        // 50% divergence
        let local = SyncHandshake::new([1; 32], 50, 5, vec![]);
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        assert!((calculate_divergence(&local, &remote) - 0.5).abs() < f64::EPSILON);

        // 100% divergence (local empty)
        let local = SyncHandshake::new([1; 32], 0, 0, vec![]);
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        assert!((calculate_divergence(&local, &remote) - 1.0).abs() < f64::EPSILON);

        // Handles zero remote count
        let local = SyncHandshake::new([1; 32], 100, 5, vec![]);
        let remote = SyncHandshake::new([2; 32], 0, 0, vec![]);
        assert!((calculate_divergence(&local, &remote) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_select_protocol_rule1_already_synced() {
        let local = SyncHandshake::new([42; 32], 100, 5, vec![]);
        let remote = SyncHandshake::new([42; 32], 200, 3, vec![]); // Same root hash

        let selection = select_protocol(&local, &remote);
        assert!(matches!(selection.protocol, SyncProtocol::None));
        assert!(selection.reason.contains("already in sync"));
    }

    #[test]
    fn test_select_protocol_rule2_fresh_node_gets_snapshot() {
        let local = SyncHandshake::new([0; 32], 0, 0, vec![]); // Fresh node
        let remote = SyncHandshake::new([42; 32], 200, 5, vec![]);

        let selection = select_protocol(&local, &remote);
        assert!(matches!(selection.protocol, SyncProtocol::Snapshot { .. }));
        assert!(selection.reason.contains("fresh node"));
    }

    #[test]
    fn test_select_protocol_rule3_initialized_node_never_gets_snapshot() {
        // CRITICAL TEST for Invariant I5
        let local = SyncHandshake::new([1; 32], 1, 1, vec![]); // Has state!
        let remote = SyncHandshake::new([42; 32], 200, 5, vec![]);

        let selection = select_protocol(&local, &remote);
        // Even with high divergence, should NOT get Snapshot
        assert!(!matches!(selection.protocol, SyncProtocol::Snapshot { .. }));
    }

    #[test]
    fn test_select_protocol_rule3_high_divergence_uses_hash_comparison() {
        let local = SyncHandshake::new([1; 32], 10, 2, vec![]); // Has state
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]); // 90% divergence

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("divergence"));
    }

    #[test]
    fn test_select_protocol_rule4_deep_tree_uses_subtree_prefetch() {
        let local = SyncHandshake::new([1; 32], 90, 5, vec![]); // ~10% divergence
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]); // depth > 3

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::SubtreePrefetch { .. }
        ));
        assert!(selection.reason.contains("subtree"));
    }

    #[test]
    fn test_select_protocol_rule5_large_tree_small_diff_uses_bloom() {
        let local = SyncHandshake::new([1; 32], 95, 2, vec![]); // ~5% divergence
        let remote = SyncHandshake::new([2; 32], 100, 2, vec![]); // entity_count > 50

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::BloomFilter { .. }
        ));
        assert!(selection.reason.contains("bloom"));
    }

    #[test]
    fn test_select_protocol_rule6_wide_shallow_uses_levelwise() {
        // Wide shallow tree: max_depth <= 2, many children per level, entity_count <= 50
        // (to avoid triggering bloom filter rule first)
        let local = SyncHandshake::new([1; 32], 40, 2, vec![]);
        let remote = SyncHandshake::new([2; 32], 40, 2, vec![]);

        let selection = select_protocol(&local, &remote);
        assert!(matches!(selection.protocol, SyncProtocol::LevelWise { .. }));
        assert!(selection.reason.contains("level"));
    }

    #[test]
    fn test_select_protocol_rule7_default_uses_hash_comparison() {
        // Create conditions that don't match any specific rule
        let local = SyncHandshake::new([1; 32], 30, 2, vec![]); // ~25% divergence
        let remote = SyncHandshake::new([2; 32], 40, 3, vec![]); // depth=3, not >3

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("default"));
    }

    #[test]
    fn test_select_protocol_version_mismatch_uses_safe_fallback() {
        let local = SyncHandshake::new([1; 32], 100, 5, vec![]);
        let mut remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        remote.version = SYNC_PROTOCOL_VERSION + 1; // Incompatible version

        let selection = select_protocol(&local, &remote);
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("version mismatch"));
    }

    #[test]
    fn test_is_protocol_supported() {
        let caps = SyncCapabilities::default();

        // Supported (in default list)
        assert!(is_protocol_supported(&SyncProtocol::None, &caps));
        assert!(is_protocol_supported(
            &SyncProtocol::HashComparison {
                root_hash: [0; 32],
                divergent_subtrees: vec![]
            },
            &caps
        ));

        // Not supported (not in default list)
        assert!(!is_protocol_supported(
            &SyncProtocol::SubtreePrefetch {
                subtree_roots: vec![]
            },
            &caps
        ));
        assert!(!is_protocol_supported(
            &SyncProtocol::LevelWise { max_depth: 2 },
            &caps
        ));
    }

    #[test]
    fn test_select_protocol_with_fallback() {
        let local = SyncHandshake::new([1; 32], 90, 5, vec![]); // Would prefer SubtreePrefetch
        let remote = SyncHandshake::new([2; 32], 100, 5, vec![]);
        let caps = SyncCapabilities::default(); // Doesn't support SubtreePrefetch

        let selection = select_protocol_with_fallback(&local, &remote, &caps);

        // Should fall back to HashComparison since SubtreePrefetch not supported
        assert!(matches!(
            selection.protocol,
            SyncProtocol::HashComparison { .. }
        ));
        assert!(selection.reason.contains("fallback"));
    }
}
