//! Per-apply context for namespace-op handlers (#2481).
//!
//! Mirrors `ops/group/context.rs` for the namespace governance path.
//! Holds the store + namespace_id; per-handler services
//! (`MetaRepository`, `MembershipRepository`, `NamespaceRepository`,
//! `NamespaceMembershipService`, etc.) are constructed on demand
//! rather than pre-built — namespace handlers don't share a fixed
//! set of helpers the way the group-op handlers do.

use crate::authorizer::AtCutAuthorizer;
use crate::{MembershipError, MembershipRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

pub(crate) struct NamespaceApplyCtx<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
    /// The applied op's causal cut (parent op hashes), for at-cut authorization
    /// (F5 #28). Empty outside the live apply path.
    parents: &'a [[u8; 32]],
    /// The at-cut apply-auth decision source (F5 #28). The default
    /// [`LiveFallbackAuthorizer`](crate::authorizer::LiveFallbackAuthorizer)
    /// always returns `None`, so gates fall back to the live resolver.
    authorizer: &'a dyn AtCutAuthorizer,
    /// Op-events queued during apply, flushed AFTER the namespace op-log
    /// entry is persisted (#2770). Handlers MUST queue_event rather than
    /// calling op_events::notify directly, or they reintroduce the
    /// emit-before-persist race.
    pending_events: Vec<crate::op_events::OpEvent>,
}

impl<'a> NamespaceApplyCtx<'a> {
    pub(crate) fn new(
        store: &'a Store,
        namespace_id: [u8; 32],
        parents: &'a [[u8; 32]],
        authorizer: &'a dyn AtCutAuthorizer,
    ) -> Self {
        Self {
            store,
            namespace_id,
            parents,
            authorizer,
            pending_events: Vec::new(),
        }
    }

    pub(crate) fn store(&self) -> &'a Store {
        self.store
    }

    pub(crate) fn namespace_id(&self) -> [u8; 32] {
        self.namespace_id
    }

    pub(crate) fn queue_event(&mut self, event: crate::op_events::OpEvent) {
        self.pending_events.push(event);
    }

    /// Drains and returns all queued events, leaving the internal buffer
    /// empty (destructive). Calling this a second time returns an empty
    /// `Vec` — the post-persist flush must `take` exactly once.
    pub(crate) fn take_events(&mut self) -> Vec<crate::op_events::OpEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Convenience: assert `signer` is a namespace-root admin or
    /// bail with `MembershipError::NotAdmin`. Hoisted from
    /// `NamespaceGovernance::require_namespace_admin` so handlers
    /// living in `ops/namespace/` don't need to thread the
    /// authorization helper through every call.
    ///
    /// Resolves through the at-cut [`AtCutAuthorizer`] FIRST (F5 #28): when the
    /// projection has folded the op's cited ancestry it answers authoritatively AS
    /// OF the op's parents (causal-honor); when it hasn't (`None`) the live resolver
    /// decides, as before. The default `LiveFallbackAuthorizer` always returns
    /// `None`, so the live path is byte-identical until a projection-backed
    /// authorizer is injected.
    pub(crate) fn require_namespace_admin(&self, signer: &PublicKey) -> EyreResult<()> {
        let ns_gid = ContextGroupId::from(self.namespace_id);
        let authorized = match self
            .authorizer
            .is_admin_at_cut(&ns_gid, signer, self.parents)
        {
            Some(verdict) => verdict,
            None => MembershipRepository::new(self.store).is_admin(&ns_gid, signer)?,
        };
        if !authorized {
            bail!(MembershipError::NotAdmin {
                group_id: hex::encode(self.namespace_id),
                identity: format!("{signer}"),
            });
        }
        Ok(())
    }

    /// The PROJECTION's at-cut membership PATH for `member` in `group`, for the
    /// `MemberJoinedOpen` gate (F5 #29b flip): `membership_path_at_cut` at the op's
    /// causal cut, validated divergence-free on the `membership-path` plane. `None`
    /// (no apply-auth context — sign/test — or an incomplete fold) means the caller
    /// must fall back to live `check_path`. Returning the `Option` (rather than taking
    /// an eager `live_path`) lets the caller compute live LAZILY — only when the
    /// projection abstains — so a `check_path` store error can't abort an apply the
    /// projection would have decided.
    pub(crate) fn projection_membership_path(
        &self,
        group: &ContextGroupId,
        member: &PublicKey,
    ) -> Option<crate::authorizer::AtCutMembershipPath> {
        self.authorizer
            .membership_path_at_cut(group, member, self.parents)
    }
}
