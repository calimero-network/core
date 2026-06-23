use crate::authorizer::AtCutAuthorizer;
use crate::MembershipRepository;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{CapabilitiesError, MembershipError};

/// Authorization service for group governance operations.
///
/// This object centralizes permission checks so callers can express intent
/// (`require_manage_members`, `require_can_create_context`) instead of wiring
/// capability bits and error messages at each callsite.
pub struct PermissionChecker<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
    /// The applied op's causal cut (parent op hashes), for the at-cut apply-auth
    /// shadow (F5 #28 stage 4). Empty outside the group-op apply path.
    parents: &'a [[u8; 32]],
    /// The at-cut apply-auth decision source (F5 #28 stage 4). The default
    /// [`LiveFallbackAuthorizer`](crate::authorizer::LiveFallbackAuthorizer) returns
    /// `None`, so the shadow no-ops for non-apply constructions (handler pre-checks,
    /// cascade pre-scans, tests).
    authorizer: &'a dyn AtCutAuthorizer,
}

impl<'a> PermissionChecker<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        Self {
            store,
            group_id,
            parents: &[],
            authorizer: &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
        }
    }

    /// Attach the op's causal cut + the at-cut apply-auth source for the group-op
    /// apply path (F5 #28 stage 4). With it, the admin/capability gates SHADOW the
    /// projection verdict against live (plane `group-auth`); without it (the
    /// default) the shadow is inert.
    #[must_use]
    pub fn with_apply_auth(
        mut self,
        parents: &'a [[u8; 32]],
        authorizer: &'a dyn AtCutAuthorizer,
    ) -> Self {
        self.parents = parents;
        self.authorizer = authorizer;
        self
    }

    pub fn is_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        // Issue #2256: admin authority cascades into Open subgroups
        // from any ancestor where the signer is a direct admin.
        // Uses `is_inherited_admin` (a dedicated walk) rather than
        // `check_group_membership_path` because the latter
        // short-circuits to `Direct` as soon as the identity has any
        // direct membership row in the target subgroup — even a
        // non-admin `Member` row — which would suppress inherited
        // admin authority for parent admins who happen to also be
        // explicit subgroup members.
        let live =
            MembershipRepository::new(self.store).is_inherited_admin(&self.group_id, identity)?;
        // SHADOW (F5 #28 stage 4): compare the projection's at-cut admin verdict to
        // live and log a `group-auth` divergence; still ACT on live. Inert unless an
        // apply-path authorizer is attached. The flip (act on projection) is stage 4b.
        self.shadow_admin(identity, live);
        Ok(live)
    }

    /// Emit a `group-auth` divergence if the at-cut projection admin verdict differs
    /// from the live `live_verdict`. `None` from the authorizer (no apply-auth
    /// context, or an incomplete fold) skips — there's nothing to compare.
    fn shadow_admin(&self, identity: &PublicKey, live_verdict: bool) {
        if let Some(projected) =
            self.authorizer
                .is_admin_at_cut(&self.group_id, identity, self.parents)
        {
            if projected != live_verdict {
                tracing::warn!(
                    marker = "unified_projection_divergence",
                    plane = "group-auth",
                    gate = "admin",
                    group_id = ?self.group_id,
                    %identity,
                    projected,
                    live = live_verdict,
                    "group apply-auth: projection admin verdict differs from live"
                );
            }
        }
    }

    pub fn require_admin(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_admin(identity)? {
            return Ok(());
        }
        // `is_admin` (via `is_inherited_admin`) is a strict superset of
        // the direct admin check, including the `GroupMeta.admin_identity`
        // fallback. Falling through to `membership.require_admin` here
        // would just re-run `is_group_admin` to format an error. Bail
        // directly with the same shape `require_group_admin` uses, so
        // callers that match on `MembershipError::NotAdmin` keep working.
        bail!(MembershipError::NotAdmin {
            group_id: format!("{:?}", self.group_id),
            identity: format!("{identity:?}"),
        });
    }

    pub fn require_manage_members(&self, identity: &PublicKey, operation: &str) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_MEMBERS)? {
            return Ok(());
        }
        // `is_authorized_with_capability` is a strict superset of the
        // direct admin-or-cap check, so falling through to
        // `require_group_admin_or_capability` would just redo the same
        // store reads to format an error. Bail directly with the same
        // diagnostic shape.
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: operation.to_owned(),
        });
    }

    pub fn require_manage_application(
        &self,
        identity: &PublicKey,
        operation: &str,
    ) -> EyreResult<()> {
        if self.can_manage_application(identity)? {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: operation.to_owned(),
        });
    }

    /// Non-bailing mirror of [`require_manage_application`]. Returns
    /// `Ok(true)` iff `identity` would pass the
    /// `MANAGE_APPLICATION` capability gate on `self.group_id` (direct
    /// admin / capability holder, or inherited admin via the Open
    /// chain). Used by the cascade apply arms to pre-scan every matched
    /// descendant before issuing any writes, so a per-descendant cap
    /// mismatch can't leave the store in a partial-cascade state.
    pub fn can_manage_application(&self, identity: &PublicKey) -> EyreResult<bool> {
        self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_APPLICATION)
    }

    pub fn require_can_create_context(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::CAN_CREATE_CONTEXT)? {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: "register context (CAN_CREATE_CONTEXT)".into(),
        })
    }

    /// `self.group_id` is the *parent* group here: a creator may make a
    /// subgroup under it if they are an admin (direct or inherited via the
    /// Open chain) or hold `CAN_CREATE_SUBGROUP`. Callers that enforce the
    /// root-level scoping of that capability (`execute_group_created`,
    /// `create_group`) layer the `parent == namespace_root` check on top.
    pub fn require_can_create_subgroup(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::CAN_CREATE_SUBGROUP)? {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: "create subgroup (CAN_CREATE_SUBGROUP)".into(),
        })
    }

    /// `self.group_id` is the namespace root: a member may cascade-delete a
    /// subgroup if they are a root admin or hold `CAN_DELETE_SUBGROUP`.
    pub fn require_can_delete_subgroup(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::CAN_DELETE_SUBGROUP)? {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: "delete subgroup (CAN_DELETE_SUBGROUP)".into(),
        })
    }

    /// `self.group_id` is the subgroup whose visibility is being changed.
    pub fn require_can_manage_visibility(&self, identity: &PublicKey) -> EyreResult<()> {
        if self
            .is_authorized_with_capability(identity, MemberCapabilities::CAN_MANAGE_VISIBILITY)?
        {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: "change subgroup visibility (CAN_MANAGE_VISIBILITY)".into(),
        })
    }

    /// Allow if `identity` is a group admin (incl. inherited admin) or holds
    /// `CAN_MANAGE_METADATA` for `self.group_id`. Used by the `*MetadataSet`
    /// ops (a member setting *their own* member metadata bypasses this — see
    /// the apply path).
    pub fn require_can_manage_metadata(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(identity, MemberCapabilities::CAN_MANAGE_METADATA)? {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: "change group metadata (CAN_MANAGE_METADATA)".into(),
        })
    }

    /// Resolves "admin or holds `capability_bit`" with Open-subgroup
    /// inheritance applied (issue #2256).
    ///
    /// Direct authority in `self.group_id` short-circuits. Otherwise:
    ///
    /// - **Admins** at any ancestor in the Open chain inherit governance
    ///   authority unconditionally (mirrors the structural-inheritance
    ///   model for parent admins).
    /// - **Non-admin** inherited members do **not** inherit governance
    ///   capabilities (`MANAGE_MEMBERS`, `MANAGE_APPLICATION`,
    ///   `CAN_CREATE_CONTEXT`, `CAN_INVITE_MEMBERS`, etc.). Their
    ///   cross-boundary authority is scoped to *context join/read* via
    ///   `CAN_JOIN_OPEN_SUBGROUPS` — the bit that already gated their
    ///   passing the membership walk in
    ///   [`super::membership::check_group_membership_path`]. Inheriting
    ///   arbitrary parent-level capabilities into the subgroup would be
    ///   a privilege-escalation path: a parent member with
    ///   `MANAGE_MEMBERS` at the namespace could otherwise add/remove
    ///   members in every Open subgroup, even though the subgroup admin
    ///   may not have intended to delegate that authority.
    ///
    /// Subgroup admins must grant governance capabilities explicitly at
    /// the subgroup level for non-admin parent members.
    fn is_authorized_with_capability(
        &self,
        identity: &PublicKey,
        capability_bit: u32,
    ) -> EyreResult<bool> {
        let direct = MembershipRepository::new(self.store).is_admin_or_has_capability(
            &self.group_id,
            identity,
            capability_bit,
        )?;
        // Only admin-inherited authority crosses the parent boundary;
        // non-admin caps must be explicit at the subgroup level.
        // Uses `is_inherited_admin` (a dedicated walk) rather than
        // `check_group_membership_path`'s `Inherited{via_admin:true}`
        // branch — the path walker short-circuits to `Direct` as soon
        // as any direct membership row exists in the target subgroup,
        // which would mask inherited admin authority for a parent
        // admin who is also an explicit non-admin subgroup member.
        let live = direct
            || MembershipRepository::new(self.store)
                .is_inherited_admin(&self.group_id, identity)?;
        // SHADOW (F5 #28 stage 4): compare the projection's at-cut admin-or-capability
        // verdict to live (plane `group-auth`); still ACT on live. Inert without an
        // apply-path authorizer. Flip is stage 4b.
        if let Some(projected) = self.authorizer.is_admin_or_capability_at_cut(
            &self.group_id,
            identity,
            capability_bit,
            self.parents,
        ) {
            if projected != live {
                tracing::warn!(
                    marker = "unified_projection_divergence",
                    plane = "group-auth",
                    gate = "capability",
                    group_id = ?self.group_id,
                    %identity,
                    capability_bit,
                    projected,
                    live,
                    "group apply-auth: projection capability verdict differs from live"
                );
            }
        }
        Ok(live)
    }

    pub fn require_admin_to_add_admin(
        &self,
        signer: &PublicKey,
        role: &GroupMemberRole,
    ) -> EyreResult<()> {
        if *role == GroupMemberRole::Admin && !self.is_admin(signer)? {
            bail!(MembershipError::NotAdmin {
                group_id: format!("{:?}", self.group_id),
                identity: format!("{signer:?}"),
            });
        }
        Ok(())
    }

    pub fn require_admin_to_remove_admin(
        &self,
        signer: &PublicKey,
        member: &PublicKey,
    ) -> EyreResult<()> {
        if self.is_admin(member)? && !self.is_admin(signer)? {
            bail!(MembershipError::NotAdmin {
                group_id: format!("{:?}", self.group_id),
                identity: format!("{signer:?}"),
            });
        }
        Ok(())
    }

    pub fn require_admin_or_self(&self, signer: &PublicKey, member: &PublicKey) -> EyreResult<()> {
        if !self.is_admin(signer)? && *signer != *member {
            bail!(CapabilitiesError::Unauthorized {
                group_id: format!("{:?}", self.group_id),
                operation: "set member alias (admin or self only)".into(),
            });
        }
        Ok(())
    }
}
