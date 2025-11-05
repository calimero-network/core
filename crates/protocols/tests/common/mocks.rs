//! Mock implementations for testing protocols

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use calimero_protocols::p2p::delta_request::{AddDeltaResult, DeltaStore, MissingParentsResult};
use calimero_storage::interface::Action;
use eyre::Result;

/// Mock DeltaStore for testing
#[derive(Clone, Default)]
pub struct MockDeltaStore {
    /// Stored deltas
    deltas: Arc<Mutex<HashMap<[u8; 32], calimero_dag::CausalDelta<Vec<Action>>>>>,
    /// Applied deltas
    applied: Arc<Mutex<Vec<[u8; 32]>>>,
    /// Events associated with deltas
    events: Arc<Mutex<HashMap<[u8; 32], Vec<u8>>>>,
    /// Missing parent IDs to simulate
    simulate_missing: Arc<Mutex<Vec<[u8; 32]>>>,
}

impl MockDeltaStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set missing parent IDs to simulate
    pub fn set_missing_parents(&self, missing: Vec<[u8; 32]>) {
        *self.simulate_missing.lock().unwrap() = missing;
    }

    /// Get all applied delta IDs
    pub fn get_applied(&self) -> Vec<[u8; 32]> {
        self.applied.lock().unwrap().clone()
    }

    /// Check if delta was applied
    pub fn is_applied(&self, delta_id: &[u8; 32]) -> bool {
        self.applied.lock().unwrap().contains(delta_id)
    }
}

#[async_trait(?Send)]
impl DeltaStore for MockDeltaStore {
    async fn has_delta(&self, delta_id: &[u8; 32]) -> bool {
        self.deltas.lock().unwrap().contains_key(delta_id)
    }

    async fn add_delta(&self, delta: calimero_dag::CausalDelta<Vec<Action>>) -> Result<()> {
        let delta_id = delta.id;
        self.deltas.lock().unwrap().insert(delta_id, delta);
        self.applied.lock().unwrap().push(delta_id);
        Ok(())
    }

    async fn add_delta_with_events(
        &self,
        delta: calimero_dag::CausalDelta<Vec<Action>>,
        events: Option<Vec<u8>>,
    ) -> Result<AddDeltaResult> {
        let delta_id = delta.id;

        // Store delta
        self.deltas.lock().unwrap().insert(delta_id, delta);

        // Store events if provided
        if let Some(events_data) = events {
            self.events.lock().unwrap().insert(delta_id, events_data);
        }

        // Check if all parents exist (simulate cascade logic)
        let has_all_parents = true; // Simplified for mock

        if has_all_parents {
            self.applied.lock().unwrap().push(delta_id);
        }

        Ok(AddDeltaResult {
            applied: has_all_parents,
            cascaded_events: vec![], // Simplified for mock
        })
    }

    async fn get_delta(
        &self,
        delta_id: &[u8; 32],
    ) -> Option<calimero_dag::CausalDelta<Vec<Action>>> {
        self.deltas.lock().unwrap().get(delta_id).cloned()
    }

    async fn get_missing_parents(&self) -> MissingParentsResult {
        MissingParentsResult {
            missing_ids: self.simulate_missing.lock().unwrap().clone(),
            cascaded_events: vec![],
        }
    }

    async fn dag_has_delta_applied(&self, delta_id: &[u8; 32]) -> bool {
        self.applied.lock().unwrap().contains(delta_id)
    }
}
