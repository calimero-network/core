//! DAG persistence and recovery tests
//!
//! Tests that DAG state (heads, pending deltas, applied deltas) can be
//! persisted and restored across restarts. This is critical for preventing
//! sync failures and timeouts after node restarts.

use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_dag::{CausalDelta, DagStats};
use calimero_storage::action::Action;

/// Serializable DAG state for persistence
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
struct PersistedDagState {
    /// Current DAG heads
    heads: Vec<[u8; 32]>,

    /// All deltas we've seen (ID -> delta)
    deltas: HashMap<[u8; 32], CausalDelta<Vec<Action>>>,

    /// IDs of applied deltas
    applied_ids: Vec<[u8; 32]>,

    /// Pending deltas (waiting for parents)
    pending_ids: Vec<[u8; 32]>,
}

impl PersistedDagState {
    fn from_stats(
        heads: Vec<[u8; 32]>,
        deltas: Vec<CausalDelta<Vec<Action>>>,
        applied_ids: Vec<[u8; 32]>,
        pending_ids: Vec<[u8; 32]>,
    ) -> Self {
        Self {
            heads,
            deltas: deltas.into_iter().map(|d| (d.id, d)).collect(),
            applied_ids,
            pending_ids,
        }
    }

    fn serialize(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
        Ok(borsh::to_vec(self)?)
    }

    fn deserialize(data: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(borsh::from_slice(data)?)
    }
}

// ============================================================
// DAG State Serialization Tests
// ============================================================

#[test]
fn test_dag_state_serialization() {
    let delta1 = CausalDelta::new(
        [1; 32],
        vec![[0; 32]],
        vec![Action::Add {
            id: calimero_storage::address::Id::new([10; 32]),
            data: b"test".to_vec(),
            ancestors: vec![],
            metadata: calimero_storage::entities::Metadata::default(),
        }],
        1000,
    );

    let state = PersistedDagState::from_stats(
        vec![[1; 32]],
        vec![delta1.clone()],
        vec![[0; 32], [1; 32]],
        vec![],
    );

    // Serialize
    let serialized = state.serialize().unwrap();
    assert!(serialized.len() > 0);

    // Deserialize
    let deserialized = PersistedDagState::deserialize(&serialized).unwrap();

    // Verify
    assert_eq!(deserialized.heads, vec![[1; 32]]);
    assert_eq!(deserialized.applied_ids, vec![[0; 32], [1; 32]]);
    assert_eq!(deserialized.deltas.get(&[1; 32]), Some(&delta1));
}

#[test]
fn test_dag_state_with_pending_deltas() {
    let delta1 = CausalDelta::new(
        [2; 32],
        vec![[1; 32]], // Missing parent
        vec![],
        2000,
    );

    let state = PersistedDagState::from_stats(
        vec![[0; 32]], // Still at root (delta1 not applied)
        vec![delta1],
        vec![[0; 32]],
        vec![[2; 32]], // delta1 is pending
    );

    // Round-trip
    let serialized = state.serialize().unwrap();
    let deserialized = PersistedDagState::deserialize(&serialized).unwrap();

    assert_eq!(deserialized.heads, vec![[0; 32]]);
    assert_eq!(deserialized.pending_ids, vec![[2; 32]]);
    assert_eq!(deserialized.applied_ids, vec![[0; 32]]);
}

// ============================================================
// DAG Restore Simulation Tests
// ============================================================

#[test]
fn test_dag_restore_preserves_heads() {
    // Simulate state before shutdown
    let heads_before = vec![[1; 32], [2; 32]]; // Two concurrent heads

    let state = PersistedDagState::from_stats(
        heads_before.clone(),
        vec![],
        vec![[0; 32], [1; 32], [2; 32]],
        vec![],
    );

    // Serialize (as if saving to DB)
    let persisted = state.serialize().unwrap();

    // Restore (as if loading from DB)
    let restored = PersistedDagState::deserialize(&persisted).unwrap();

    // Heads preserved
    assert_eq!(restored.heads, heads_before);
}

#[test]
fn test_dag_restore_preserves_pending() {
    // State with pending deltas
    let pending_delta = CausalDelta::new(
        [5; 32],
        vec![[4; 32]], // Missing parent
        vec![],
        5000,
    );

    let state = PersistedDagState::from_stats(
        vec![[1; 32]],
        vec![pending_delta.clone()],
        vec![[0; 32], [1; 32]],
        vec![[5; 32]],
    );

    // Persist and restore
    let serialized = state.serialize().unwrap();
    let restored = PersistedDagState::deserialize(&serialized).unwrap();

    // Pending delta preserved
    assert_eq!(restored.pending_ids, vec![[5; 32]]);
    assert_eq!(restored.deltas.get(&[5; 32]), Some(&pending_delta));
}

#[test]
fn test_dag_restore_large_state() {
    // Simulate large DAG state (100 deltas, 20 pending)
    let mut deltas = Vec::new();
    let mut applied_ids = vec![[0; 32]];

    for i in 1..=100 {
        let id = {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes
        };

        let parent_id = if i > 80 {
            // Last 20 are pending
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(999_u64).to_le_bytes()); // Missing parent
            bytes
        } else {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&((i - 1) as u64).to_le_bytes());
            bytes
        };

        deltas.push(CausalDelta::new(
            id,
            vec![parent_id],
            vec![],
            i as u64 * 1000,
        ));

        if i <= 80 {
            applied_ids.push(id);
        }
    }

    let pending_ids: Vec<_> = (81..=100)
        .map(|i| {
            let mut bytes = [0u8; 32];
            bytes[0..8].copy_from_slice(&(i as u64).to_le_bytes());
            bytes
        })
        .collect();

    let heads = vec![{
        let mut bytes = [0u8; 32];
        bytes[0..8].copy_from_slice(&(80_u64).to_le_bytes());
        bytes
    }];

    let state = PersistedDagState::from_stats(heads, deltas, applied_ids, pending_ids);

    // Serialize large state
    let serialized = state.serialize().unwrap();
    assert!(serialized.len() > 1000, "Should be substantial data");

    // Restore
    let restored = PersistedDagState::deserialize(&serialized).unwrap();

    // Verify counts
    assert_eq!(restored.heads.len(), 1);
    assert_eq!(restored.applied_ids.len(), 81); // root + 80
    assert_eq!(restored.pending_ids.len(), 20);
    assert_eq!(restored.deltas.len(), 100);
}

// ============================================================
// Recovery Scenario Tests
// ============================================================

#[test]
fn test_recovery_scenario_mid_sync() {
    // Scenario: Node was syncing, had pending deltas, then crashed
    let pending1 = CausalDelta::new([10; 32], vec![[9; 32]], vec![], 10000);
    let pending2 = CausalDelta::new([11; 32], vec![[10; 32]], vec![], 11000);
    let pending3 = CausalDelta::new([12; 32], vec![[11; 32]], vec![], 12000);

    let state = PersistedDagState::from_stats(
        vec![[5; 32]], // Last applied
        vec![pending1.clone(), pending2.clone(), pending3.clone()],
        vec![[0; 32], [1; 32], [2; 32], [3; 32], [4; 32], [5; 32]],
        vec![[10; 32], [11; 32], [12; 32]],
    );

    // After restore, pending deltas should be available
    let restored = PersistedDagState::deserialize(&state.serialize().unwrap()).unwrap();

    // When parent [9; 32] arrives, these can all apply
    assert_eq!(restored.pending_ids.len(), 3);
    assert!(restored.deltas.contains_key(&[10; 32]));
    assert!(restored.deltas.contains_key(&[11; 32]));
    assert!(restored.deltas.contains_key(&[12; 32]));
}

#[test]
fn test_recovery_scenario_concurrent_branches() {
    // Scenario: Node had multiple concurrent heads when it crashed
    let heads = vec![[10; 32], [20; 32], [30; 32]];

    let deltas = vec![
        CausalDelta::new([10; 32], vec![[0; 32]], vec![], 1000),
        CausalDelta::new([20; 32], vec![[0; 32]], vec![], 1001),
        CausalDelta::new([30; 32], vec![[0; 32]], vec![], 1002),
    ];

    let state = PersistedDagState::from_stats(
        heads.clone(),
        deltas.clone(),
        vec![[0; 32], [10; 32], [20; 32], [30; 32]],
        vec![],
    );

    // Restore
    let restored = PersistedDagState::deserialize(&state.serialize().unwrap()).unwrap();

    // Multiple heads preserved
    let mut restored_heads = restored.heads.clone();
    restored_heads.sort();
    let mut expected_heads = heads;
    expected_heads.sort();

    assert_eq!(restored_heads, expected_heads);
}
