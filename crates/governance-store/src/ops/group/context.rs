//! Per-apply context for group-op handlers.
//!
//! Holds everything a group-op handler needs to do its work:
//! - `store` / `group_id` / `signer` for store I/O and authorization
//! - Pre-built service helpers (`permissions`, `membership_policy`,
//!   `settings`, `context_registration`) so each op doesn't
//!   re-instantiate them
//! - `divergence` — out-parameter for ops that compute a post-apply
//!   state-hash check (`MemberRemoved`, `MemberLeft`). Set by those
//!   handlers; left `None` by every other op.
//!
//! Field visibility note: only `divergence` is `pub(crate)` mutable.
//! Every other field is private and exposed through read-only
//! accessor methods. This keeps handlers from accidentally
//! re-binding the authorization context (`signer`, `group_id`,
//! `store`) within a dispatch call.

use crate::{
    ContextRegistrationService, DivergenceReport, GroupSettingsService, MembershipPolicy,
    PermissionChecker,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;

pub(crate) struct GroupApplyCtx<'a> {
    store: &'a Store,
    group_id: &'a ContextGroupId,
    signer: &'a PublicKey,
    permissions: PermissionChecker<'a>,
    membership_policy: MembershipPolicy<'a>,
    settings: GroupSettingsService<'a>,
    context_registration: ContextRegistrationService<'a>,
    /// Populated by post-apply hash-check arms (`MemberRemoved`,
    /// `MemberLeft`) when the recomputed local state diverges from
    /// the signed claim. The dispatcher forwards this up the apply
    /// pipeline; the node-side handler routes it to the
    /// reconcile-via-anchor path.
    pub(crate) divergence: Option<DivergenceReport>,
    /// Op-events queued during apply, flushed by the caller AFTER the
    /// op-log entry is persisted (see #2770). Mirrors `divergence`'s
    /// out-parameter pattern. Handlers MUST `queue_event` rather than
    /// calling `op_events::notify` directly, or they reintroduce the
    /// emit-before-persist race.
    pub(crate) pending_events: Vec<crate::op_events::OpEvent>,
}

impl<'a> GroupApplyCtx<'a> {
    /// Construct the per-apply context with the op's causal cut + at-cut authorizer,
    /// so the `PermissionChecker` admin/capability gates resolve against the
    /// projection at the cut, with live as the `None`-fallback (F5 #28 stage 4). Pass
    /// `&[]` + `LIVE_FALLBACK_AUTHORIZER` for constructions without an apply-auth
    /// context (they then use the live resolver).
    pub(crate) fn new_with_apply_auth(
        store: &'a Store,
        group_id: &'a ContextGroupId,
        signer: &'a PublicKey,
        parents: &'a [[u8; 32]],
        authorizer: &'a dyn crate::authorizer::AtCutAuthorizer,
    ) -> Self {
        Self {
            store,
            group_id,
            signer,
            permissions: PermissionChecker::new(store, *group_id)
                .with_apply_auth(parents, authorizer),
            membership_policy: MembershipPolicy::new(store, *group_id)
                .with_apply_auth(parents, authorizer),
            settings: GroupSettingsService::new(store, *group_id),
            context_registration: ContextRegistrationService::new(store, *group_id),
            divergence: None,
            pending_events: Vec::new(),
        }
    }

    pub(crate) fn queue_event(&mut self, event: crate::op_events::OpEvent) {
        self.pending_events.push(event);
    }

    pub(crate) fn store(&self) -> &'a Store {
        self.store
    }

    pub(crate) fn group_id(&self) -> &'a ContextGroupId {
        self.group_id
    }

    pub(crate) fn signer(&self) -> &'a PublicKey {
        self.signer
    }

    pub(crate) fn permissions(&self) -> &PermissionChecker<'a> {
        &self.permissions
    }

    pub(crate) fn membership_policy(&self) -> &MembershipPolicy<'a> {
        &self.membership_policy
    }

    pub(crate) fn settings(&self) -> &GroupSettingsService<'a> {
        &self.settings
    }

    pub(crate) fn context_registration(&self) -> &ContextRegistrationService<'a> {
        &self.context_registration
    }
}
