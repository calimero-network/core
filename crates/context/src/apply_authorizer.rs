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
/// ephemeral projection of the op's namespace, at the op's causal cut.
pub struct EphemeralProjectionAuthorizer<'a> {
    store: &'a Store,
}

impl<'a> EphemeralProjectionAuthorizer<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
}

impl AtCutAuthorizer for EphemeralProjectionAuthorizer<'_> {
    fn is_admin_at_cut(
        &self,
        group: ContextGroupId,
        signer: &PublicKey,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // Fold the namespace's persisted governance DAG once, then resolve the
        // admin verdict AT the op's parent cut. `None` (store fault, or the cited
        // ancestry not fully folded) makes the gate defer to the live resolver.
        let (proj, _ns, _heads) = ScopeProjections::ephemeral_projection(self.store, &group)?;
        proj.is_admin_at_cut(self.store, group, signer, parents)
    }
}
