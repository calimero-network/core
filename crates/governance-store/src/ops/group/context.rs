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
use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

pub(crate) struct GroupApplyCtx<'a> {
    store: &'a Store,
    group_id: &'a ContextGroupId,
    signer: &'a PublicKey,
    /// The op's causal cut, and the source that resolves authority against it.
    ///
    /// Held directly — not only baked into the sub-services below — because a gate
    /// may need to ask about an identity that is neither the signer nor a
    /// membership mutation target. The account plane's link gate is the case:
    /// it asks whether an *account's root key* is a member, which is nobody's
    /// `PermissionChecker` question.
    parents: &'a [[u8; 32]],
    authorizer: &'a dyn crate::authorizer::AtCutAuthorizer,
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
            parents,
            authorizer,
            permissions: PermissionChecker::new(store, *group_id)
                .with_apply_auth(parents, authorizer),
            membership_policy: MembershipPolicy::new(store, *group_id)
                .with_apply_auth(parents, authorizer),
            settings: GroupSettingsService::new(store, *group_id)
                .with_apply_auth(parents, authorizer),
            context_registration: ContextRegistrationService::new(store, *group_id),
            divergence: None,
            pending_events: Vec::new(),
        }
    }

    pub(crate) fn queue_event(&mut self, event: crate::op_events::OpEvent) {
        self.pending_events.push(event);
    }

    /// Queue the member-facing migration announcement for an upgrade op.
    ///
    /// `to_state_version` is `None` for ops that don't carry one, in which case
    /// it comes off the local application row. Every lookup here is
    /// best-effort: an announcement is observational, so a store error must
    /// degrade the event rather than fail the apply and diverge this replica.
    pub(crate) fn queue_migration_started(
        &mut self,
        previous_application_id: &ApplicationId,
        target_application_id: &ApplicationId,
        to_state_version: Option<u32>,
        local_contexts_total: u32,
    ) {
        // Bundle ids are version-stable and every ladder rung names the same one,
        // so an equal "from" id means the row already holds the NEW version.
        let from_version = if previous_application_id == target_application_id {
            "unknown".to_owned()
        } else {
            application_version(self.store, previous_application_id)
        };
        let to_state_version = to_state_version
            .or_else(|| {
                self.store
                    .handle()
                    .get(&calimero_store::key::ApplicationMeta::new(
                        *target_application_id,
                    ))
                    .ok()
                    .flatten()
                    .map(|app| app.state_version)
            })
            .unwrap_or_default();
        let to_version = application_version(self.store, target_application_id);
        self.queue_event(crate::op_events::OpEvent::MigrationStarted {
            group_id: self.group_id.to_bytes(),
            from_version,
            to_version,
            to_state_version,
            local_contexts_total,
        });
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

    /// The account the op's signer speaks for, or `None` if the key is bound
    /// to none here.
    ///
    /// Handlers that ask a "did the signer act on their own behalf" question —
    /// a self-leave, a self-metadata edit, a self auto-follow flip — compare
    /// this against the account the op names. The comparison cannot be made on
    /// the key: an account is a hash and nothing signs as one.
    ///
    /// `None` is always a refusal, never a fallback. A key bound to no account
    /// here is not the member it claims to be, so it cannot act as them.
    ///
    /// Resolved through the checker rather than off the binding rows directly,
    /// because those rows fold like any other projected state. A bare live read
    /// makes the answer depend on how far THIS replica has folded, so a peer
    /// that has the signer's device-link op and one that does not would reach
    /// opposite verdicts on the same signed self-op — one applies it, the other
    /// refuses it, and nothing later reconciles them. The checker parks that op
    /// instead, and a park is retried once the ancestry arrives.
    pub(crate) fn signer_account(&self) -> EyreResult<Option<AccountId>> {
        self.permissions.account_for_signer(self.signer)
    }

    /// Whether the op's signer is acting on their own behalf — the shared
    /// self-check behind every "admin or self" gate. See
    /// [`signer_account`](Self::signer_account) for why `None` refuses.
    pub(crate) fn signer_is(&self, member: &AccountId) -> EyreResult<bool> {
        Ok(self.signer_account()?.as_ref() == Some(member))
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

    /// The op's causal cut — its parent op hashes.
    ///
    /// Exposed so a gate can *report* which cut it decided against. An authority
    /// decision that differs between two replicas is only debuggable if the cut it
    /// was made at is in the log beside the verdict.
    pub(crate) const fn cut(&self) -> &'a [[u8; 32]] {
        self.parents
    }

    /// The projection's at-cut membership path for `member` in this group.
    ///
    /// `None` means the projection has no verdict — either there is no cut to
    /// resolve against, or its ancestry is not folded here. The two are NOT
    /// interchangeable, so a caller that falls back to live MUST first call
    /// [`ensure_live_fallback_is_sound`](Self::ensure_live_fallback_is_sound).
    ///
    /// Returns the `Option` rather than taking an eagerly-computed live path so
    /// the caller can compute live lazily, and a live store error cannot abort an
    /// apply the projection would have decided on its own.
    pub(crate) fn projection_membership_path(
        &self,
        member: &AccountId,
    ) -> Option<crate::authorizer::AtCutMembershipPath> {
        self.authorizer
            .membership_path_at_cut(self.group_id, member, self.parents)
    }

    /// Guard a fall-back to the live resolver, mirroring
    /// `PermissionChecker::ensure_live_fallback_is_sound`.
    ///
    /// The at-cut resolver abstains for two reasons and collapsing them is what
    /// makes apply-time authority replica-dependent. When there is no cut (a
    /// genesis op, or a construction with no apply-auth context at all — the emit
    /// path, local apply, tests) live is the right answer, because no causal
    /// context contradicts it. When the cut is real but unfolded here, live is a
    /// *different* cut, so the verdict would turn on how much this replica
    /// happens to have folded and two replicas would decide the same op
    /// differently. In that case refuse to answer; the op is retried once the
    /// missing history arrives.
    pub(crate) fn ensure_live_fallback_is_sound(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.authorizer.can_resolve_cut(self.group_id, self.parents) {
            return Ok(());
        }
        bail!(crate::ApplyError::AuthorityUndecidable {
            group_id: format!("{:?}", self.group_id),
            signer: format!("{identity}"),
        });
    }
}

/// The semver string on `id`'s local application row, or `"unknown"`.
///
/// Nothing on the wire carries these, so every migration announcement resolves
/// them against this node's own rows. One reader keeps the announcement and the
/// per-descendant cascade record from falling back differently.
pub(crate) fn application_version(store: &Store, id: &ApplicationId) -> String {
    store
        .handle()
        .get(&calimero_store::key::ApplicationMeta::new(*id))
        .ok()
        .flatten()
        .map_or_else(|| "unknown".to_owned(), |app| String::from(app.version))
}
