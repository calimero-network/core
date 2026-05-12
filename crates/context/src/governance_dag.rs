//! DAG-based governance: applies [`SignedGroupOp`] and [`SignedNamespaceOp`]
//! in causal order.

use std::sync::{Arc, Mutex};

use calimero_context_client::local_governance::{SignedGroupOp, SignedNamespaceOp};
use calimero_dag::{ApplyError, CausalDelta, DeltaApplier};
use calimero_store::Store;

use crate::group_store;
use crate::group_store::DivergenceReport;

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
    Ok(make_delta(
        op,
        op.parent_op_hashes.clone(),
        op.state_hash,
        delta_id,
    ))
}

// ---------------------------------------------------------------------------
// Namespace governance DAG
// ---------------------------------------------------------------------------

/// Applies a [`SignedNamespaceOp`] to the persistent namespace store.
///
/// Implements [`DeltaApplier`] so `DagStore<SignedNamespaceOp>` can delegate
/// application to namespace-aware store logic.
///
/// Carries an outbox slot for the divergence report produced by
/// `MemberRemoved` / `MemberLeft` apply: the `DeltaApplier::apply`
/// trait returns `Result<(), ApplyError>` and has no room for
/// structured output, so the report gets stashed here and the
/// handler reads-and-clears it after the DAG `add_delta` call
/// returns. Single-flight per applier instance (one `add_delta`
/// inside one actor mailbox slot), so the slot is safe against the
/// concurrent-clobber case.
pub struct NamespaceGovernanceApplier {
    store: Store,
    divergence_outbox: Arc<Mutex<Option<DivergenceReport>>>,
}

impl NamespaceGovernanceApplier {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            divergence_outbox: Arc::new(Mutex::new(None)),
        }
    }

    /// Read and clear the outbox. Called by the handler after
    /// `add_delta_with_outcome` returns to retrieve any divergence
    /// the apply path detected.
    pub fn take_divergence(&self) -> Option<DivergenceReport> {
        self.divergence_outbox
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }
}

#[async_trait::async_trait]
impl DeltaApplier<SignedNamespaceOp> for NamespaceGovernanceApplier {
    async fn apply(&self, delta: &CausalDelta<SignedNamespaceOp>) -> Result<(), ApplyError> {
        let outcome = group_store::apply_signed_namespace_op(&self.store, &delta.payload)
            .map_err(|e| ApplyError::Application(e.to_string()))?;
        if let Some(report) = outcome.divergence {
            // Last-writer-wins on the outbox. The applier instance
            // is single-flight per actor message turn, so multiple
            // writes here would only happen if a single
            // `add_delta_with_outcome` call ran multiple group ops
            // (which it doesn't in current call shapes). If a
            // future change introduces that, the handler will see
            // the last report — preferable to silently dropping all
            // but the first.
            if let Ok(mut slot) = self.divergence_outbox.lock() {
                *slot = Some(report);
            }
        }
        Ok(())
    }
}

/// Build a [`CausalDelta`] from a [`SignedNamespaceOp`] for insertion into the
/// namespace governance DAG.
pub fn signed_namespace_op_to_delta(
    op: &SignedNamespaceOp,
) -> Result<CausalDelta<SignedNamespaceOp>, eyre::Error> {
    let delta_id = op
        .content_hash()
        .map_err(|e| eyre::eyre!("content_hash: {e}"))?;
    Ok(make_delta(
        op,
        op.parent_op_hashes.clone(),
        op.state_hash,
        delta_id,
    ))
}

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
