//! Projection-backed [`AtCutAuthorizer`] for the apply gates (F5 #28 stage 3b/4).
//!
//! Realizes the dependency inversion opened by the governance-store seam: the
//! apply gates call the `AtCutAuthorizer` trait (defined in `governance-store`),
//! and this implementation resolves the decision against the unified projection at
//! the op's causal cut, falling back to the live resolver (`None`) when the cited
//! ancestry isn't fully folded.
//!
//! It folds an EPHEMERAL projection from the store — the same mechanism the
//! read-side query gates use ([`ScopeProjections::ephemeral_projection`]) — so it
//! needs no shared projection state threaded into the apply path. The fold is cached
//! per authorizer (i.e. per apply): one op may hit several gates (admin, capability,
//! last-admin) but touches one namespace, so the DAG is walked once. The fold is
//! bounded by `MAX_BACKFILL_OPS`; governance ops are infrequent, and P6 sync
//! unification replaces the ephemeral fold with the maintained projection.

use std::sync::{Arc, Mutex, PoisonError};

use calimero_context_config::types::ContextGroupId;
use calimero_governance_store::AtCutAuthorizer;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;

use crate::scope_projection::ScopeProjections;

/// The cached ephemeral fold: the folded projection plus the namespace id + heads
/// `ScopeProjections::ephemeral_projection` returns (kept so the shape matches even
/// though the at-cut reads take the op's own `parents`, not these heads).
type FoldedProjection = (ScopeProjections, [u8; 32], Vec<[u8; 32]>);

/// An [`AtCutAuthorizer`] that resolves each gate against an ephemeral projection of
/// the op's namespace, at the op's causal cut. Constructed at the namespace DAG
/// applier (the one call site); `pub(crate)` because it's an implementation detail,
/// not API other crates should depend on.
pub(crate) struct EphemeralProjectionAuthorizer<'a> {
    store: &'a Store,
    /// Per-apply fold cache (see module doc). Keyed by `group`: every gate for one op
    /// passes the same group, so after the first fold the rest are cache hits; a
    /// different group re-folds. `Mutex` (not `RefCell`) because the trait is
    /// `Send + Sync` and drives a spawned `async fn apply` future.
    cache: Mutex<Option<(ContextGroupId, Arc<FoldedProjection>)>>,
}

impl<'a> EphemeralProjectionAuthorizer<'a> {
    pub(crate) fn new(store: &'a Store) -> Self {
        Self {
            store,
            cache: Mutex::new(None),
        }
    }

    /// Fold the ephemeral projection for `group`'s namespace ONCE and cache it, so
    /// the several gates a single op hits reuse the fold instead of re-walking the
    /// DAG each time. `None` = the fold couldn't be built (store/DAG-head fault) —
    /// warned, and the gate defers to live; the expected "ancestry not yet folded"
    /// case instead surfaces as `None` from the at-cut read on a successful fold.
    fn folded(&self, group: &ContextGroupId) -> Option<Arc<FoldedProjection>> {
        let mut slot = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((cached_group, folded)) = slot.as_ref() {
            if cached_group == group {
                return Some(Arc::clone(folded));
            }
        }
        let Some(folded) = ScopeProjections::ephemeral_projection(self.store, group) else {
            tracing::warn!(
                group = ?group,
                "apply-auth: ephemeral projection unavailable (store/DAG-head fault); gate defers to live"
            );
            return None;
        };
        let arc = Arc::new(folded);
        *slot = Some((*group, Arc::clone(&arc)));
        Some(arc)
    }
}

impl AtCutAuthorizer for EphemeralProjectionAuthorizer<'_> {
    fn is_admin_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        // `None` here = the cited ancestry isn't fully folded; the gate defers to
        // live (quiet — a normal mid-backfill state, not an error).
        self.folded(group)?
            .0
            .is_admin_at_cut(self.store, *group, signer, parents)
    }

    fn is_admin_or_capability_at_cut(
        &self,
        group: &ContextGroupId,
        signer: &PublicKey,
        capability: u32,
        parents: &[[u8; 32]],
    ) -> Option<bool> {
        self.folded(group)?
            .0
            .is_admin_or_capability_at_cut(self.store, *group, signer, capability, parents)
    }
}
