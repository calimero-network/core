//! DAG-based group governance: applies [`SignedGroupOp`] in causal order.

use calimero_context_primitives::local_governance::SignedGroupOp;
use calimero_dag::{ApplyError, CausalDelta, DeltaApplier};
use calimero_store::Store;

use crate::group_store;

/// Applies a [`SignedGroupOp`] to the persistent group store.
///
/// Implements [`DeltaApplier`] so `DagStore<SignedGroupOp>` can delegate
/// application to the existing `apply_local_signed_group_op` logic.
pub struct GroupGovernanceApplier {
    store: Store,
}

impl GroupGovernanceApplier {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl DeltaApplier<SignedGroupOp> for GroupGovernanceApplier {
    async fn apply(&self, delta: &CausalDelta<SignedGroupOp>) -> Result<(), ApplyError> {
        group_store::apply_local_signed_group_op(&self.store, &delta.payload)
            .map_err(|e| ApplyError::Application(e.to_string()))
    }
}

/// Build a [`CausalDelta`] from a [`SignedGroupOp`] for insertion into the DAG.
///
/// `delta_id` = content hash of the op.
/// `parents` = the op's `parent_op_hashes`.
pub fn signed_op_to_delta(op: &SignedGroupOp) -> Result<CausalDelta<SignedGroupOp>, eyre::Error> {
    let delta_id = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    Ok(CausalDelta::new(
        delta_id,
        op.parent_op_hashes.clone(),
        op.clone(),
        // HLC is not used for governance ordering (nonce + DAG parents suffice);
        // default is acceptable since DagStore uses parents for topological sort.
        calimero_storage::logical_clock::HybridTimestamp::default(),
        // Governance ops have no Merkle root; zero hash signals "no state hash".
        [0u8; 32],
    ))
}
