use crate::authorizer::AtCutAuthorizer;
use crate::MembershipRepository;
use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_context_config::MemberCapabilities;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{ApplyError, CapabilitiesError, MembershipError};

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

    /// Guard the live fallback.
    ///
    /// The at-cut resolver abstains for two very different reasons, and collapsing
    /// them is what made apply-time authority replica-dependent:
    ///
    /// - There is no cut to resolve against (a genesis op's empty `parents`, or a
    ///   construction with no apply-auth context at all — the emit path, the local
    ///   apply, the read side, tests). Live is the right answer; nothing contradicts it.
    ///
    /// - The cut is real, but this node has not folded its ancestry. Live is the WRONG
    ///   answer: it is a different cut — this replica's current one — so the verdict
    ///   would turn on how much this replica happens to have folded. A replica that had
    ///   folded a concurrent capability revoke would reject an op its peers applied, and
    ///   because the reject path never advances the DAG head, everything descending from
    ///   that op would stall on that replica alone.
    ///
    /// In the second case, refuse to answer rather than guess. The op is retried once
    /// the missing history arrives.
    fn ensure_live_fallback_is_sound(&self, identity: &PublicKey) -> EyreResult<()> {
        if self
            .authorizer
            .can_resolve_cut(&self.group_id, self.parents)
        {
            return Ok(());
        }
        bail!(ApplyError::AuthorityUndecidable {
            group_id: format!("{:?}", self.group_id),
            signer: format!("{identity}"),
        });
    }

    /// [`ensure_live_fallback_is_sound`](Self::ensure_live_fallback_is_sound)
    /// for a gate whose subject is an account rather than a signing key.
    fn ensure_live_fallback_is_sound_for_account(&self, member: &AccountId) -> EyreResult<()> {
        if self
            .authorizer
            .can_resolve_cut(&self.group_id, self.parents)
        {
            return Ok(());
        }
        bail!(ApplyError::AuthorityUndecidable {
            group_id: format!("{:?}", self.group_id),
            signer: format!("{member:?}"),
        });
    }

    /// The account `identity` speaks for on the live path, or `None`.
    ///
    /// The two authorization paths ask about the same signing key but resolve
    /// it differently, and that is deliberate: the at-cut path resolves through
    /// the projection folded to the op's own parents, while the live fallback
    /// resolves through the materialized binding rows. Each must resolve in its
    /// own frame — using live bindings to decide an at-cut question is the
    /// divergence [`Self::ensure_live_fallback_is_sound`] exists to prevent.
    ///
    /// `None` means the key is bound to no account here (never enrolled, or
    /// revoked), and every caller reads that as "not authorized". Failing
    /// closed matters more here than anywhere: a key-derived stand-in would
    /// name a principal that holds no grant, so the gate would refuse anyway —
    /// but only after writing the refusal into a shape that looks like a real
    /// verdict about a real account.
    fn live_account(&self, identity: &PublicKey) -> EyreResult<Option<AccountId>> {
        crate::member_account_in_namespace(self.store, &self.group_id, identity)
    }

    /// Has this replica learned who holds authority in this namespace yet?
    ///
    /// Before genesis applies, the root meta carries
    /// [`crate::PLACEHOLDER_ADMIN_IDENTITY`] and no binding has been written by
    /// any join — so the namespace has no authority to check against, and every
    /// answer this checker could give is about its own sync progress rather than
    /// about the op.
    fn authority_established(&self) -> EyreResult<bool> {
        let root = crate::NamespaceRepository::new(self.store).resolve(&self.group_id)?;
        Ok(crate::MetaRepository::new(self.store)
            .load(&root)?
            .is_some_and(|meta| meta.admin_identity != crate::placeholder_admin_identity()))
    }

    /// The account `identity` speaks for, or a PARK when this replica cannot yet
    /// say and cannot honestly answer "no" either.
    ///
    /// "Bound to no account here" has two very different causes, and the key
    /// alone does not distinguish them: a stranger who holds authority nowhere,
    /// or a real member whose binding this replica has not folded yet. Answering
    /// `false` for the second turns a timing gap into a permanent verdict — the
    /// publisher authorized its own op from live rows and accepted it, the
    /// receiver drops it, and no later op reconciles the two.
    ///
    /// The tie is broken on whether this namespace has any authority established
    /// at all. Before genesis there is nothing to have been a stranger TO, so the
    /// op parks and is retried once the ancestry arrives. After genesis the rows
    /// are meaningful and an unresolvable signer is genuinely unauthorized —
    /// which also keeps a forged op signed by an unbound key from stalling the
    /// DAG, since parking on it would be a denial of service.
    fn live_account_or_park(&self, identity: &PublicKey) -> EyreResult<Option<AccountId>> {
        if let Some(account) = self.live_account(identity)? {
            return Ok(Some(account));
        }
        if self.authority_established()? {
            return Ok(None);
        }
        bail!(ApplyError::AuthorityUndecidable {
            group_id: format!("{:?}", self.group_id),
            signer: format!("{identity}"),
        })
    }

    pub fn is_admin(&self, identity: &PublicKey) -> EyreResult<bool> {
        // Decide from the PROJECTION at the op's causal cut — admin authority as of the
        // op's own parents, which is the same answer on every replica.
        if let Some(verdict) =
            self.authorizer
                .is_admin_at_cut(&self.group_id, identity, self.parents)
        {
            return Ok(verdict);
        }
        self.ensure_live_fallback_is_sound(identity)?;
        let Some(account) = self.live_account_or_park(identity)? else {
            return Ok(false);
        };
        // Issue #2256: admin authority cascades into Open subgroups
        // from any ancestor where the signer is a direct admin.
        // Uses `is_inherited_admin` (a dedicated walk) rather than
        // `check_group_membership_path` because the latter
        // short-circuits to `Direct` as soon as the identity has any
        // direct membership row in the target subgroup — even a
        // non-admin `Member` row — which would suppress inherited
        // admin authority for parent admins who happen to also be
        // explicit subgroup members.
        MembershipRepository::new(self.store).is_inherited_admin(&self.group_id, &account)
    }

    /// Is `member` an admin? The account-typed sibling of
    /// [`is_admin`](Self::is_admin), for gates that ask about the op's TARGET.
    ///
    /// It resolves at the cut exactly as the signer form does. Answering this
    /// one from live while the signer half resolved at the cut would make a
    /// single gate straddle two cuts, which is the divergence
    /// [`ensure_live_fallback_is_sound`](Self::ensure_live_fallback_is_sound)
    /// exists to prevent.
    pub fn is_admin_account(&self, member: &AccountId) -> EyreResult<bool> {
        if let Some(verdict) =
            self.authorizer
                .is_admin_account_at_cut(&self.group_id, member, self.parents)
        {
            return Ok(verdict);
        }
        self.ensure_live_fallback_is_sound_for_account(member)?;
        MembershipRepository::new(self.store).is_inherited_admin(&self.group_id, member)
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

    /// Non-bailing mirror of [`require_manage_application`](Self::require_manage_application).
    /// Returns `Ok(true)` iff `identity` would pass the `MANAGE_APPLICATION` capability
    /// gate on `self.group_id` (direct admin / capability holder, or inherited admin
    /// via the Open chain).
    ///
    /// The cascade arms no longer pre-scan descendants with this: they authorize ONCE
    /// against the root, because re-deriving authority per descendant made the verdict
    /// depend on each replica's fold progress and diverged the cluster.
    pub fn can_manage_application(&self, identity: &PublicKey) -> EyreResult<bool> {
        self.is_authorized_with_capability(identity, MemberCapabilities::MANAGE_APPLICATION.bits())
    }

    /// Bool analogue of [`require_can_create_subgroup`](Self::require_can_create_subgroup),
    /// for callers that combine it with the root-level scoping check rather than
    /// bailing on it directly (the `GroupCreated` apply arm).
    pub fn can_create_subgroup(&self, identity: &PublicKey) -> EyreResult<bool> {
        self.is_authorized_with_capability(identity, MemberCapabilities::CAN_CREATE_SUBGROUP.bits())
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
        // Decide from the PROJECTION at the op's causal cut (the capability analogue
        // of `is_admin`). Capabilities are exactly what concurrent
        // `MemberCapabilitySet` / `DefaultCapabilitiesSet` / `MemberRoleSet` ops
        // mutate, so a live read here is the tightest version of the divergence loop:
        // the gate for a capability op would depend on capabilities this replica may
        // or may not have folded yet.
        if let Some(verdict) = self.authorizer.is_admin_or_capability_at_cut(
            &self.group_id,
            identity,
            capability_bit,
            self.parents,
        ) {
            return Ok(verdict);
        }
        self.ensure_live_fallback_is_sound(identity)?;
        let Some(account) = self.live_account_or_park(identity)? else {
            return Ok(false);
        };
        let direct = MembershipRepository::new(self.store).is_admin_or_has_capability(
            &self.group_id,
            &account,
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
                .is_inherited_admin(&self.group_id, &account)?)
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
        member: &AccountId,
    ) -> EyreResult<()> {
        if self.is_admin_account(member)? && !self.is_admin(signer)? {
            bail!(MembershipError::NotAdmin {
                group_id: format!("{:?}", self.group_id),
                identity: format!("{signer:?}"),
            });
        }
        Ok(())
    }

    /// An admin, or the member acting on their own behalf.
    ///
    /// The self-check crosses the key/account boundary — `signer` is a key, and
    /// `member` names the principal the row belongs to — so it resolves rather
    /// than comparing. It used to be `*signer != *member` with both sides keys,
    /// which kept compiling after the flip because both ids are 32 bytes: the
    /// comparison simply stopped ever being true, silently narrowing this gate
    /// to admins only.
    pub fn require_admin_or_self(&self, signer: &PublicKey, member: &AccountId) -> EyreResult<()> {
        let is_self = self.live_account(signer)?.as_ref() == Some(member);
        if !is_self && !self.is_admin(signer)? {
            bail!(CapabilitiesError::Unauthorized {
                group_id: format!("{:?}", self.group_id),
                operation: "set member alias (admin or self only)".into(),
            });
        }
        Ok(())
    }
}
