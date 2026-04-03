//! DAG-based governance: applies [`SignedGroupOp`] and [`SignedNamespaceOp`]
//! in causal order.

use calimero_context_client::local_governance::{SignedGroupOp, SignedNamespaceOp};
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
    GovernanceDeltaFactory::from_signed_group_op(op)
}

// ---------------------------------------------------------------------------
// Namespace governance DAG
// ---------------------------------------------------------------------------

/// Applies a [`SignedNamespaceOp`] to the persistent namespace store.
///
/// Implements [`DeltaApplier`] so `DagStore<SignedNamespaceOp>` can delegate
/// application to namespace-aware store logic.
pub struct NamespaceGovernanceApplier {
    store: Store,
}

impl NamespaceGovernanceApplier {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl DeltaApplier<SignedNamespaceOp> for NamespaceGovernanceApplier {
    async fn apply(&self, delta: &CausalDelta<SignedNamespaceOp>) -> Result<(), ApplyError> {
        let _pending = group_store::apply_signed_namespace_op(&self.store, &delta.payload)
            .map_err(|e| ApplyError::Application(e.to_string()))?;
        Ok(())
    }
}

/// Build a [`CausalDelta`] from a [`SignedNamespaceOp`] for insertion into the
/// namespace governance DAG.
pub fn signed_namespace_op_to_delta(
    op: &SignedNamespaceOp,
) -> Result<CausalDelta<SignedNamespaceOp>, eyre::Error> {
    GovernanceDeltaFactory::from_signed_namespace_op(op)
}

/// Factory for converting signed governance operations into DAG deltas.
pub struct GovernanceDeltaFactory;

impl GovernanceDeltaFactory {
    fn make_delta<T>(
        op: &T,
        parents: Vec<[u8; 32]>,
        expected_root_hash: [u8; 32],
        delta_id: [u8; 32],
    ) -> CausalDelta<T>
    where
        T: Clone,
    {
        CausalDelta::new(
            delta_id,
            parents,
            op.clone(),
            calimero_storage::logical_clock::HybridTimestamp::default(),
            expected_root_hash,
        )
    }

    /// Build a `CausalDelta` for a signed group operation.
    pub fn from_signed_group_op(
        op: &SignedGroupOp,
    ) -> Result<CausalDelta<SignedGroupOp>, eyre::Error> {
        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        Ok(Self::make_delta(
            op,
            op.parent_op_hashes.clone(),
            op.state_hash,
            delta_id,
        ))
    }

    /// Build a `CausalDelta` for a signed namespace operation.
    pub fn from_signed_namespace_op(
        op: &SignedNamespaceOp,
    ) -> Result<CausalDelta<SignedNamespaceOp>, eyre::Error> {
        let delta_id = op
            .content_hash()
            .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
        Ok(Self::make_delta(
            op,
            op.parent_op_hashes.clone(),
            op.state_hash,
            delta_id,
        ))
    }
}
