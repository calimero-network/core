//! DAG-based governance: applies [`SignedGroupOp`] and [`SignedNamespaceOp`]
//! in causal order.
use std::sync::{Arc, Mutex};

use calimero_context_client::local_governance::{SignedGroupOp, SignedNamespaceOp};
use calimero_dag::{ApplyError, CausalDelta, DeltaApplier};
use calimero_store::Store;

use calimero_governance_store;
use calimero_governance_store::DivergenceReport;

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
        // F5 #28 stage 4: the STANDALONE group-op DAG keeps the LIVE gates. A
        // `SignedGroupOp`'s `parent_op_hashes` live in the per-group op log, NOT the
        // namespace governance log the projection is keyed by — so handing them to
        // `EphemeralProjectionAuthorizer` (a namespace-projection resolver) would have
        // it treat group-DAG hashes as namespace delta ids, fail `cut_ancestry_complete`,
        // and silently no-op to live anyway. The real `group-auth` shadow/flip runs on
        // the namespace-ENVELOPE group-op path (`NamespaceGovernance` decrypt-and-apply),
        // where the cut is the enclosing namespace op's parents (correct id-space).
        calimero_governance_store::apply_local_signed_group_op(&self.store, &delta.payload)
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
    ///
    /// Recovers from a poisoned mutex via `into_inner` instead of
    /// discarding the report on poison: the outbox is plain
    /// `Option<DivergenceReport>` with no internal invariants a panic
    /// could leave half-written, so the slot's value is still
    /// well-formed. Dropping it silently would mean the reconcile
    /// path never fires on the report a poison-inducing panic was
    /// concurrent with — exactly the operator-investigation signal
    /// we need to preserve.
    pub fn take_divergence(&self) -> Option<DivergenceReport> {
        let mut slot = self.divergence_outbox.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                "divergence_outbox mutex was poisoned by a prior panic; recovering the \
                     inner value so the divergence report still reaches the reconcile path"
            );
            poisoned.into_inner()
        });
        slot.take()
    }
}

#[async_trait::async_trait]
impl DeltaApplier<SignedNamespaceOp> for NamespaceGovernanceApplier {
    async fn apply(&self, delta: &CausalDelta<SignedNamespaceOp>) -> Result<(), ApplyError> {
        // F5 #28 (stage 3b): authorize the apply gates against the PROJECTION at the
        // op's causal cut. The ephemeral authorizer folds the namespace's persisted
        // governance DAG and resolves admin authority as of `delta.parents`; on an
        // incomplete fold it returns `None` and the gate falls back to the live
        // resolver, so a cold/racing fold never wrongly rejects a valid op.
        let authorizer = crate::apply_authorizer::EphemeralProjectionAuthorizer::new(&self.store);
        let outcome = calimero_governance_store::apply_signed_namespace_op_at_cut(
            &self.store,
            &delta.payload,
            &delta.parents,
            &authorizer,
        )
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
            //
            // Mutex poison: recover the inner slot rather than drop
            // the report. The slot is plain `Option<_>`; no half-
            // written invariants for a panic to leave behind. Losing
            // the divergence here would mean the reconcile path
            // never fires on this op.
            let mut slot = self.divergence_outbox.lock().unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "divergence_outbox mutex was poisoned by a prior panic; recovering \
                         the inner slot so this divergence report still reaches the \
                         reconcile path"
                );
                poisoned.into_inner()
            });
            *slot = Some(report);
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
