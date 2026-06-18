//! Per-apply context for namespace-op handlers (#2481).
//!
//! Mirrors `ops/group/context.rs` for the namespace governance path.
//! Holds the store + namespace_id; per-handler services
//! (`MetaRepository`, `MembershipRepository`, `NamespaceRepository`,
//! `NamespaceMembershipService`, etc.) are constructed on demand
//! rather than pre-built — namespace handlers don't share a fixed
//! set of helpers the way the group-op handlers do.

use crate::{MembershipError, MembershipRepository};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

pub(crate) struct NamespaceApplyCtx<'a> {
    store: &'a Store,
    namespace_id: [u8; 32],
    /// Op-events queued during apply, flushed AFTER the namespace op-log
    /// entry is persisted (#2770). Handlers MUST queue_event rather than
    /// calling op_events::notify directly, or they reintroduce the
    /// emit-before-persist race.
    pending_events: Vec<crate::op_events::OpEvent>,
}

impl<'a> NamespaceApplyCtx<'a> {
    pub(crate) fn new(store: &'a Store, namespace_id: [u8; 32]) -> Self {
        Self {
            store,
            namespace_id,
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
    pub(crate) fn require_namespace_admin(&self, signer: &PublicKey) -> EyreResult<()> {
        let ns_gid = ContextGroupId::from(self.namespace_id);
        if !MembershipRepository::new(self.store).is_admin(&ns_gid, signer)? {
            bail!(MembershipError::NotAdmin {
                group_id: hex::encode(self.namespace_id),
                identity: format!("{signer}"),
            });
        }
        Ok(())
    }
}
