//! Sync handshake protocol types (CIP §2 - Sync Handshake Protocol).
//!
//! Types for initial sync negotiation between peers.

use borsh::{BorshDeserialize, BorshSerialize};

use super::protocol::{SyncProtocol, SyncProtocolKind};

// =============================================================================
// Constants
// =============================================================================

/// Wire protocol version for sync handshake.
///
/// Increment on breaking changes to ensure nodes can detect incompatibility.
pub const SYNC_PROTOCOL_VERSION: u32 = 1;

// =============================================================================
// Capabilities
// =============================================================================

/// Capabilities advertised during sync negotiation.
///
/// Used to determine mutually supported features between peers.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SyncCapabilities {
    /// Whether compression is supported.
    pub supports_compression: bool,
    /// Maximum entities per batch transfer.
    pub max_batch_size: u64,
    /// Protocols this node supports (ordered by preference).
    pub supported_protocols: Vec<SyncProtocolKind>,
}

impl Default for SyncCapabilities {
    fn default() -> Self {
        Self {
            supports_compression: true,
            max_batch_size: 1000,
            supported_protocols: vec![
                SyncProtocolKind::None,
                SyncProtocolKind::DeltaSync,
                SyncProtocolKind::HashComparison,
                SyncProtocolKind::Snapshot,
            ],
        }
    }
}

// =============================================================================
// Handshake Messages
// =============================================================================

/// Sync handshake message (Initiator → Responder).
///
/// Contains the initiator's state summary for protocol negotiation.
///
/// See CIP §2.1 - Handshake Message.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SyncHandshake {
    /// Protocol version for compatibility checking.
    pub version: u32,
    /// Current Merkle root hash.
    pub root_hash: [u8; 32],
    /// Number of entities in the tree.
    pub entity_count: u64,
    /// Maximum depth of the Merkle tree.
    pub max_depth: u32,
    /// Current DAG heads (latest delta IDs).
    pub dag_heads: Vec<[u8; 32]>,
    /// Whether this node has any state.
    pub has_state: bool,
    /// Supported protocols (ordered by preference).
    pub supported_protocols: Vec<SyncProtocolKind>,
}

impl SyncHandshake {
    /// Create a new handshake message from local state.
    #[must_use]
    pub fn new(
        root_hash: [u8; 32],
        entity_count: u64,
        max_depth: u32,
        dag_heads: Vec<[u8; 32]>,
    ) -> Self {
        let has_state = root_hash != [0; 32];
        Self {
            version: SYNC_PROTOCOL_VERSION,
            root_hash,
            entity_count,
            max_depth,
            dag_heads,
            has_state,
            supported_protocols: SyncCapabilities::default().supported_protocols,
        }
    }

    /// Check if the remote handshake has a compatible protocol version.
    #[must_use]
    pub fn is_version_compatible(&self, other: &Self) -> bool {
        self.version == other.version
    }

    /// Check if root hashes match (already in sync).
    #[must_use]
    pub fn is_in_sync(&self, other: &Self) -> bool {
        self.root_hash == other.root_hash
    }
}

impl Default for SyncHandshake {
    fn default() -> Self {
        Self::new([0; 32], 0, 0, vec![])
    }
}

/// Sync handshake response (Responder → Initiator).
///
/// Contains the selected protocol and responder's state summary.
///
/// See CIP §2.2 - Negotiation Flow.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct SyncHandshakeResponse {
    /// Protocol selected for this sync session.
    pub selected_protocol: SyncProtocol,
    /// Responder's current root hash.
    pub root_hash: [u8; 32],
    /// Responder's entity count.
    pub entity_count: u64,
    /// Responder's capabilities.
    pub capabilities: SyncCapabilities,
}

impl SyncHandshakeResponse {
    /// Create a response indicating no sync is needed.
    #[must_use]
    pub fn already_synced(root_hash: [u8; 32], entity_count: u64) -> Self {
        Self {
            selected_protocol: SyncProtocol::None,
            root_hash,
            entity_count,
            capabilities: SyncCapabilities::default(),
        }
    }

    /// Create a response with a selected protocol.
    #[must_use]
    pub fn with_protocol(
        selected_protocol: SyncProtocol,
        root_hash: [u8; 32],
        entity_count: u64,
    ) -> Self {
        Self {
            selected_protocol,
            root_hash,
            entity_count,
            capabilities: SyncCapabilities::default(),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_capabilities_roundtrip() {
        let caps = SyncCapabilities::default();
        let encoded = borsh::to_vec(&caps).expect("serialize");
        let decoded: SyncCapabilities = borsh::from_slice(&encoded).expect("deserialize");
        assert_eq!(caps, decoded);
    }

    #[test]
    fn test_sync_handshake_roundtrip() {
        let handshake = SyncHandshake::new([42; 32], 100, 5, vec![[1; 32], [2; 32]]);

        let encoded = borsh::to_vec(&handshake).expect("serialize");
        let decoded: SyncHandshake = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(handshake, decoded);
        assert_eq!(decoded.version, SYNC_PROTOCOL_VERSION);
        assert!(decoded.has_state); // non-zero root_hash
    }

    #[test]
    fn test_sync_handshake_response_roundtrip() {
        let response = SyncHandshakeResponse::with_protocol(
            SyncProtocol::HashComparison {
                root_hash: [7; 32],
                divergent_subtrees: vec![],
            },
            [8; 32],
            500,
        );

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: SyncHandshakeResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
    }

    #[test]
    fn test_sync_handshake_version_compatibility() {
        let local = SyncHandshake::new([1; 32], 10, 2, vec![]);
        let compatible = SyncHandshake::new([2; 32], 20, 3, vec![]);
        let incompatible = SyncHandshake {
            version: SYNC_PROTOCOL_VERSION + 1,
            ..SyncHandshake::default()
        };

        assert!(local.is_version_compatible(&compatible));
        assert!(!local.is_version_compatible(&incompatible));
    }

    #[test]
    fn test_sync_handshake_in_sync_detection() {
        let local = SyncHandshake::new([42; 32], 100, 5, vec![]);
        let same_hash = SyncHandshake::new([42; 32], 200, 6, vec![[1; 32]]);
        let different_hash = SyncHandshake::new([99; 32], 100, 5, vec![]);

        assert!(local.is_in_sync(&same_hash));
        assert!(!local.is_in_sync(&different_hash));
    }

    #[test]
    fn test_sync_handshake_fresh_node() {
        let fresh = SyncHandshake::new([0; 32], 0, 0, vec![]);
        assert!(!fresh.has_state);

        let initialized = SyncHandshake::new([1; 32], 1, 1, vec![]);
        assert!(initialized.has_state);
    }

    #[test]
    fn test_sync_handshake_response_already_synced() {
        let response = SyncHandshakeResponse::already_synced([42; 32], 100);
        assert_eq!(response.selected_protocol, SyncProtocol::None);
    }
}
