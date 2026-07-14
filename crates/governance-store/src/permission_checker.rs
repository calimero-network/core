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
    /// The applied op's causal cut (parent op hashes), at which the apply-auth gates
    /// resolve (F5 #28 stage 4). Empty outside the group-op apply path.
    parents: &'a [[u8; 32]],
    /// The at-cut apply-auth decision source (F5 #28 stage 4). The default
    /// [`LiveFallbackAuthorizer`](crate::authorizer::LiveFallbackAuthorizer) returns
    /// `None`, so non-apply constructions (handler pre-checks, cascade pre-scans,
    /// tests) keep using the live resolver.
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
    /// apply path (F5 #28 stage 4). With it, the admin/capability gates decide from
    /// the projection at the cut (live as `None`-fallback); without it (the default)
    /// they use the live resolver.
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
        // F5 #28 stage 4b: decide from the PROJECTION at the op's causal cut — admin
        // authority as of the op's own parents (causal-honor), validated divergence-
        // free on the `group-auth` plane (stage 4a). `None` (no apply-auth context —
        // a local pre-check / cascade / test — OR an incomplete fold) falls back to
        // the live resolver. The live fallback retires in #29b.
        if let Some(verdict) =
            self.authorizer
                .is_admin_at_cut(&self.group_id, identity, self.parents)
        {
            return Ok(verdict);
        }
        // Issue #2256: admin authority cascades into Open subgroups
        // from any ancestor where the signer is a direct admin.
        // Uses `is_inherited_admin` (a dedicated walk) rather than
        // `check_group_membership_path` because the latter
        // short-circuits to `Direct` as soon as the identity has any
        // direct membership row in the target subgroup — even a
        // non-admin `Member` row — which would suppress inherited
        // admin authority for parent admins who happen to also be
        // explicit subgroup members.
        MembershipRepository::new(self.store).is_inherited_admin(&self.group_id, identity)
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
        if self
            .is_authorized_with_capability(identity, MemberCapabilities::MANAGE_MEMBERS.bits())?
        {
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
        self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_APPLICATION.bits())
    }

    pub fn require_can_create_context(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(
            identity,
            MemberCapabilities::CAN_CREATE_CONTEXT.bits(),
        )? {
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
        if self.is_authorized_with_capability(
            identity,
            MemberCapabilities::CAN_CREATE_SUBGROUP.bits(),
        )? {
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
        if self.is_authorized_with_capability(
            identity,
            MemberCapabilities::CAN_DELETE_SUBGROUP.bits(),
        )? {
            return Ok(());
        }
        bail!(CapabilitiesError::Unauthorized {
            group_id: format!("{:?}", self.group_id),
            operation: "delete subgroup (CAN_DELETE_SUBGROUP)".into(),
        })
    }

    /// `self.group_id` is the subgroup whose visibility is being changed.
    pub fn require_can_manage_visibility(&self, identity: &PublicKey) -> EyreResult<()> {
        if self.is_authorized_with_capability(
            identity,
            MemberCapabilities::CAN_MANAGE_VISIBILITY.bits(),
        )? {
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
        if self.is_authorized_with_capability(
            identity,
            MemberCapabilities::CAN_MANAGE_METADATA.bits(),
        )? {
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
        // F5 #28 stage 4b: decide from the PROJECTION at the op's causal cut (the
        // capability analogue of `is_admin`); `None` falls back to live. Validated on
        // the `group-auth` plane in stage 4a.
        if let Some(verdict) = self.authorizer.is_admin_or_capability_at_cut(
            &self.group_id,
            identity,
            capability_bit,
            self.parents,
        ) {
            return Ok(verdict);
        }
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
        Ok(direct
            || MembershipRepository::new(self.store)
                .is_inherited_admin(&self.group_id, identity)?)
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
