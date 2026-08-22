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
    Ok(make_delta(op, op.parent_op_hashes.clone(), delta_id))
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
    /// Set when the apply failed because authority was UNDECIDABLE at the cut,
    /// rather than refused. Same single-slot, read-once shape as
    /// `divergence_outbox`, and single-flight for the same reason.
    undecidable_outbox: Arc<Mutex<Option<String>>>,
}

impl NamespaceGovernanceApplier {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            divergence_outbox: Arc::new(Mutex::new(None)),
            undecidable_outbox: Arc::new(Mutex::new(None)),
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
    /// The group whose authority the last apply could not resolve, if that is why
    /// it failed — read once, like [`Self::take_divergence`].
    ///
    /// `ApplyError::Application` carries only a string, so the typed
    /// `AuthorityUndecidable` the gate raised is gone by the time the DAG returns
    /// it. It matters at the API edge, where "not yet" and "refused" want opposite
    /// client behaviour, so it is carried out of band from the one place that
    /// still has the type rather than recovered by matching on prose.
    pub fn take_undecidable(&self) -> Option<String> {
        let mut slot = self.undecidable_outbox.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                "undecidable_outbox mutex was poisoned by a prior panic; recovering the \
                 inner value so the retryable outcome still reaches the caller"
            );
            poisoned.into_inner()
        });
        slot.take()
    }

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
        .map_err(|e| {
            // The one place the typed gate error still exists. Downcast before the
            // string conversion swallows it; a `None` here is any other apply
            // failure and stays exactly as it was.
            if let Some(calimero_governance_store::ApplyError::AuthorityUndecidable {
                group_id,
                ..
            }) = e.downcast_ref::<calimero_governance_store::ApplyError>()
            {
                let mut slot = self.undecidable_outbox.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!(
                        "undecidable_outbox mutex was poisoned by a prior panic; \
                         recovering the inner slot so this retryable outcome still \
                         reaches the caller"
                    );
                    poisoned.into_inner()
                });
                *slot = Some(group_id.clone());
            }
            ApplyError::Application(e.to_string())
        })?;
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

// `signed_namespace_op_to_delta` now lives in `calimero-governance-store`
// alongside `op_from_namespace_op` (the governance apply uses both to build the
// decoded unified op it writes atomically with the gov-DAG put). Re-exported here
// unchanged so the existing callers in this crate keep using
// `crate::governance_dag::signed_namespace_op_to_delta`.
pub use calimero_governance_store::unified_op_decode::signed_namespace_op_to_delta;

fn make_delta<T>(op: &T, parents: Vec<[u8; 32]>, delta_id: [u8; 32]) -> CausalDelta<T>
where
    T: Clone,
{
    CausalDelta::new(
        delta_id,
        parents,
        op.clone(),
        calimero_storage::logical_clock::HybridTimestamp::default(),
    )
}
