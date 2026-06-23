//! Projection-backed [`AtCutAuthorizer`] for the apply gates (F5 #28 stage 3b).
//!
//! Realizes the dependency inversion opened by the governance-store seam: the
//! apply gates call the `AtCutAuthorizer` trait (defined in `governance-store`),
//! and this implementation resolves the decision against the unified projection at
//! the op's causal cut, falling back to the live resolver (`None`) when the cited
//! ancestry isn't fully folded.
//!
//! It folds an EPHEMERAL projection from the store per call — the same mechanism
//! the read-side query gates use ([`ScopeProjections::ephemeral_projection`]) — so
//! it needs no shared projection state threaded into the apply path. The fold is
//! bounded by `MAX_BACKFILL_OPS`; governance ops are infrequent, and P6 sync
//! unification removes the per-apply fold.

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::AtCutAuthorizer;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;

use crate::scope_projection::ScopeProjections;

/// An [`AtCutAuthorizer`] that resolves each gate against a freshly folded
/// ephemeral projection of the op's namespace, at the op's causal cut. Constructed
/// at the namespace DAG applier (the one call site); `pub(crate)` because it's an
/// implementation detail, not API other crates should depend on.
pub(crate) struct EphemeralProjectionAuthorizer<'a> {
    store: &'a Store,
}

impl<'a> EphemeralProjectionAuthorizer<'a> {
    pub(crate) fn new(store: &'a Store) -> Self {
        Self { store }
    }
}

impl AtCutAuthorizer for EphemeralProjectionAuthorizer<'_> {
    fn is_admin_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Fold the namespace's persisted governance DAG, then resolve the admin
        // verdict AT the op's parent cut.
        let Some((proj, _ns, _heads)) = ScopeProjections::ephemeral_projection(self.store, group)
        else {
            // The fold itself could not be built — a store/DAG-head fault, NOT the
            // expected "ancestry not yet folded" case (that surfaces as `None` from
            // `is_admin_at_cut` below and is a quiet, frequent backfill state). Defer
            // to live as the `None` contract requires, but WARN: a transient storage
            // fault shouldn't silently downgrade the apply-auth source unobserved.
            tracing::warn!(
                group = ?group,
                "apply-auth: ephemeral projection unavailable (store/DAG-head fault); gate defers to live"
            );
            return None;
        };
        // `None` here = the cited ancestry isn't fully folded; the gate defers to
        // live (quiet — a normal mid-backfill state, not an error).
        proj.is_admin_at_cut(self.store, *group, signer, parents)
    }
}
