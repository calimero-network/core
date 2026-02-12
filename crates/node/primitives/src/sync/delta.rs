//! Delta sync types (CIP §4 - State Machine, DELTA SYNC branch).
//!
//! Types for delta-based synchronization when few deltas are missing.

use borsh::{BorshDeserialize, BorshSerialize};

// =============================================================================
// Constants
// =============================================================================

/// Default threshold for choosing delta sync vs state-based sync.
///
/// If fewer than this many deltas are missing, use delta sync.
/// If more are missing, escalate to state-based sync (HashComparison, etc.).
///
/// This is a heuristic balance between:
/// - Delta sync: O(missing) round trips, but exact
/// - State sync: O(log n) comparisons, but may transfer more data
pub const DEFAULT_DELTA_SYNC_THRESHOLD: usize = 128;

// =============================================================================
// Request/Response Types
// =============================================================================

/// Request for delta-based synchronization.
///
/// Used when few deltas are missing and their IDs are known.
/// The responder should return the requested deltas in causal order.
///
/// See CIP §4 - State Machine (DELTA SYNC branch).
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaSyncRequest {
    /// IDs of missing deltas to request.
    pub missing_delta_ids: Vec<[u8; 32]>,
}

impl DeltaSyncRequest {
    /// Create a new delta sync request.
    #[must_use]
    pub fn new(missing_delta_ids: Vec<[u8; 32]>) -> Self {
        Self { missing_delta_ids }
    }

    /// Check if the request is within the recommended threshold.
    #[must_use]
    pub fn is_within_threshold(&self) -> bool {
        self.missing_delta_ids.len() <= DEFAULT_DELTA_SYNC_THRESHOLD
    }

    /// Number of deltas being requested.
    #[must_use]
    pub fn count(&self) -> usize {
        self.missing_delta_ids.len()
    }
}

/// Response to a delta sync request.
///
/// Contains the requested deltas in causal order (parents before children).
/// If some deltas are not found, they are omitted from the response.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaSyncResponse {
    /// Deltas in causal order (parents first).
    /// Each delta is serialized as bytes for transport.
    pub deltas: Vec<DeltaPayload>,

    /// IDs of deltas that were requested but not found.
    pub not_found: Vec<[u8; 32]>,
}

impl DeltaSyncResponse {
    /// Create a response with found deltas.
    #[must_use]
    pub fn new(deltas: Vec<DeltaPayload>, not_found: Vec<[u8; 32]>) -> Self {
        Self { deltas, not_found }
    }

    /// Create an empty response (no deltas found).
    #[must_use]
    pub fn empty(not_found: Vec<[u8; 32]>) -> Self {
        Self {
            deltas: vec![],
            not_found,
        }
    }

    /// Check if all requested deltas were found.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.not_found.is_empty()
    }

    /// Number of deltas returned.
    #[must_use]
    pub fn count(&self) -> usize {
        self.deltas.len()
    }
}

// =============================================================================
// Delta Payload
// =============================================================================

/// A delta payload for transport.
///
/// Contains the delta data and metadata needed for application.
#[derive(Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct DeltaPayload {
    /// Unique delta ID (content hash).
    pub id: [u8; 32],

    /// Parent delta IDs (for causal ordering).
    pub parents: Vec<[u8; 32]>,

    /// Serialized delta operations (Borsh-encoded).
    pub payload: Vec<u8>,

    /// HLC timestamp when the delta was created.
    pub hlc_timestamp: u64,

    /// Expected root hash after applying this delta.
    ///
    /// This hash is captured by the originating node at delta creation time.
    /// It serves two purposes:
    /// 1. **Linear history**: When deltas are applied in sequence without
    ///    concurrent branches, receivers can verify they reach the same state.
    /// 2. **DAG consistency**: When concurrent deltas exist, this hash ensures
    ///    nodes build identical DAG structures even if their local root hashes
    ///    differ due to different merge ordering. The hash reflects the
    ///    originator's state, not a universal truth.
    ///
    /// **Verification strategy**: Compare against this hash only when applying
    /// deltas from a single linear chain. For concurrent/merged deltas, use
    /// the Merkle tree reconciliation protocol instead.
    pub expected_root_hash: [u8; 32],
}

impl DeltaPayload {
    /// Check if this delta has no parents (genesis delta).
    #[must_use]
    pub fn is_genesis(&self) -> bool {
        self.parents.is_empty()
    }
}

// =============================================================================
// Apply Result
// =============================================================================

/// Result of attempting to apply deltas.
///
/// **Note**: This is a local-only type used to report the outcome of delta
/// application. It is not transmitted over the wire (hence no Borsh traits).
#[derive(Clone, Debug)]
pub enum DeltaApplyResult {
    /// All deltas applied successfully.
    Success {
        /// Number of deltas applied.
        applied_count: usize,
        /// New root hash after applying.
        new_root_hash: [u8; 32],
    },

    /// Some deltas could not be applied due to missing parents.
    /// Suggests escalation to state-based sync.
    MissingParents {
        /// Delta IDs whose parents are missing.
        missing_parent_deltas: Vec<[u8; 32]>,
        /// Number of deltas successfully applied before failure.
        applied_before_failure: usize,
    },

    /// Delta application failed (hash mismatch, corruption, etc.).
    Failed {
        /// Description of the failure.
        reason: String,
    },
}

impl DeltaApplyResult {
    /// Check if delta application was fully successful.
    #[must_use]
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success { .. })
    }

    /// Check if escalation to state-based sync is needed.
    #[must_use]
    pub fn needs_state_sync(&self) -> bool {
        matches!(self, Self::MissingParents { .. })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delta_sync_request_roundtrip() {
        let request = DeltaSyncRequest::new(vec![[1; 32], [2; 32], [3; 32]]);

        let encoded = borsh::to_vec(&request).expect("serialize");
        let decoded: DeltaSyncRequest = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(request, decoded);
        assert_eq!(decoded.count(), 3);
    }

    #[test]
    fn test_delta_sync_request_threshold() {
        // Within threshold
        let small_request = DeltaSyncRequest::new(vec![[1; 32]; 10]);
        assert!(small_request.is_within_threshold());

        // At threshold
        let at_threshold = DeltaSyncRequest::new(vec![[1; 32]; DEFAULT_DELTA_SYNC_THRESHOLD]);
        assert!(at_threshold.is_within_threshold());

        // Over threshold
        let large_request = DeltaSyncRequest::new(vec![[1; 32]; DEFAULT_DELTA_SYNC_THRESHOLD + 1]);
        assert!(!large_request.is_within_threshold());
    }

    #[test]
    fn test_delta_payload_roundtrip() {
        let payload = DeltaPayload {
            id: [1; 32],
            parents: vec![[2; 32], [3; 32]],
            payload: vec![4, 5, 6, 7],
            hlc_timestamp: 12345678,
            expected_root_hash: [8; 32],
        };

        let encoded = borsh::to_vec(&payload).expect("serialize");
        let decoded: DeltaPayload = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(payload, decoded);
        assert!(!decoded.is_genesis());
    }

    #[test]
    fn test_delta_payload_genesis() {
        let genesis = DeltaPayload {
            id: [1; 32],
            parents: vec![], // No parents = genesis
            payload: vec![1, 2, 3],
            hlc_timestamp: 0,
            expected_root_hash: [2; 32],
        };

        assert!(genesis.is_genesis());

        let non_genesis = DeltaPayload {
            id: [2; 32],
            parents: vec![[1; 32]], // Has parent
            payload: vec![4, 5, 6],
            hlc_timestamp: 1,
            expected_root_hash: [3; 32],
        };

        assert!(!non_genesis.is_genesis());
    }

    #[test]
    fn test_delta_sync_response_roundtrip() {
        let delta1 = DeltaPayload {
            id: [1; 32],
            parents: vec![],
            payload: vec![1, 2, 3],
            hlc_timestamp: 100,
            expected_root_hash: [10; 32],
        };
        let delta2 = DeltaPayload {
            id: [2; 32],
            parents: vec![[1; 32]],
            payload: vec![4, 5, 6],
            hlc_timestamp: 200,
            expected_root_hash: [20; 32],
        };

        let response = DeltaSyncResponse::new(vec![delta1, delta2], vec![[99; 32]]);

        let encoded = borsh::to_vec(&response).expect("serialize");
        let decoded: DeltaSyncResponse = borsh::from_slice(&encoded).expect("deserialize");

        assert_eq!(response, decoded);
        assert_eq!(decoded.count(), 2);
        assert!(!decoded.is_complete()); // Has not_found entries
    }

    #[test]
    fn test_delta_sync_response_complete() {
        let delta = DeltaPayload {
            id: [1; 32],
            parents: vec![],
            payload: vec![1, 2, 3],
            hlc_timestamp: 100,
            expected_root_hash: [10; 32],
        };

        let complete_response = DeltaSyncResponse::new(vec![delta], vec![]); // No not_found
        assert!(complete_response.is_complete());

        let incomplete_response = DeltaSyncResponse::empty(vec![[1; 32]]);
        assert!(!incomplete_response.is_complete());
        assert_eq!(incomplete_response.count(), 0);
    }

    #[test]
    fn test_delta_apply_result_success() {
        let success = DeltaApplyResult::Success {
            applied_count: 5,
            new_root_hash: [1; 32],
        };
        assert!(success.is_success());
        assert!(!success.needs_state_sync());
    }

    #[test]
    fn test_delta_apply_result_missing_parents() {
        let missing = DeltaApplyResult::MissingParents {
            missing_parent_deltas: vec![[1; 32]],
            applied_before_failure: 3,
        };
        assert!(!missing.is_success());
        assert!(missing.needs_state_sync());
    }

    #[test]
    fn test_delta_apply_result_failed() {
        let failed = DeltaApplyResult::Failed {
            reason: "hash mismatch".to_string(),
        };
        assert!(!failed.is_success());
        assert!(!failed.needs_state_sync());
    }
}
