//! At-cut apply authorization seam (F5 #28).
//!
//! The apply-time governance gates (e.g. [`NamespaceApplyCtx::require_namespace_admin`])
//! historically read the LIVE membership resolver, which has no causal context —
//! they decide against the receiver's current state, not the op's own parents. F5
//! moves that decision onto the unified projection, resolved at the op's causal
//! cut. But the projection lives in `crates/context`, which DEPENDS on this crate,
//! so this crate cannot reach it directly.
//!
//! [`AtCutAuthorizer`] inverts that dependency: this crate defines the trait the
//! apply gates call; `crates/context` implements it backed by the projection and
//! injects it at the apply call site. The gate stays here; only the decision
//! source is abstracted. `None` from any method means "the projection has no
//! authoritative verdict at this cut" (an incomplete fold) — the gate falls back
//! to the live resolver, exactly as the read-side consumers do.
//!
//! [`NamespaceApplyCtx::require_namespace_admin`]: crate::ops::namespace::NamespaceApplyCtx

use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;

/// The apply-gate decision source, resolved at an op's causal cut (its parent op
/// hashes). Implemented in `crates/context` over the unified projection; this
/// crate calls it from the apply gates without depending on the projection.
///
/// Every method returns `Option<bool>`: `Some(verdict)` when the projection's
/// cited ancestry is fully folded and the answer is authoritative, `None` when it
/// isn't (the caller defers to the live resolver).
///
/// `Send + Sync`: the apply path runs inside the namespace DAG applier's `async
/// fn apply`, whose future must be `Send` (it's spawned). A `&dyn AtCutAuthorizer`
/// is only `Send` when the trait object is `Sync`, so the bound is required for the
/// apply future — and the axum handlers that drive it — to stay `Send`.
pub trait AtCutAuthorizer: Send + Sync {
    /// Is `signer` an admin of `group` at the cut named by `parents`?
    fn is_admin_at_cut(
        &self,
        group: ContextGroupId,
        signer: &PublicKey,
        parents: &[[u8; 32]],
    ) -> Option<bool>;

    // The capability gate (`is_admin_or_capability_at_cut`) lands with its caller
    // in the next stage (the `PermissionChecker` capability-check flip), per the
    // no-premature-API convention — it isn't added here because no gate in this
    // stage calls it.
}

/// The identity authorizer: always `None`, so every gate falls back to the live
/// resolver. The default for [`NamespaceGovernance`](crate::NamespaceGovernance)
/// constructions that don't carry an apply-auth context (tests, the sign path,
/// read-only facades) and the behavior-preserving injection while the projection-
/// backed implementation is wired up but not yet authoritative.
pub struct LiveFallbackAuthorizer;

impl AtCutAuthorizer for LiveFallbackAuthorizer {
    fn is_admin_at_cut(
        &self,
        _group: ContextGroupId,
        _signer: &PublicKey,
        _parents: &[[u8; 32]],
    ) -> Option<bool> {
        None
    }
}

/// A shared `'static` [`LiveFallbackAuthorizer`] so a default `&dyn AtCutAuthorizer`
/// needs no allocation or caller-held value.
pub static LIVE_FALLBACK_AUTHORIZER: LiveFallbackAuthorizer = LiveFallbackAuthorizer;
