use crate::MembershipRepository;
use calimero_account::AccountId;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::super::{
    read_tee_admission_policy, MembershipError, TeeAdmissionPolicy, TeeAdmissionPolicyRead,
};
use super::policy_rules::{
    tcb_status_allowed, validate_tee_attestation_allowlists, MembershipPolicyRejection,
    MembershipPolicyValidationError, TeeAllowlistPolicy, TeeAttestationClaims, TEE_REJECT_MRTD,
    TEE_REJECT_MRTD_EMPTY, TEE_REJECT_RTMR0, TEE_REJECT_RTMR1, TEE_REJECT_RTMR1_EMPTY,
    TEE_REJECT_RTMR2, TEE_REJECT_RTMR2_EMPTY, TEE_REJECT_RTMR3, TEE_REJECT_RTMR3_EMPTY,
    TEE_REJECT_TCB_STATUS,
};
use super::view::GroupMembershipView;
use crate::metrics::record_membership_policy_rejection;

/// Membership policy service for governance mutations.
///
/// Encapsulates business rules around admin cardinality and TEE admission
/// allowlists so mutation handlers can stay focused on state transitions.
pub struct MembershipPolicy<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
    membership: GroupMembershipView<'a>,
    /// The applied op's causal cut (parent op hashes), at which the last-admin
    /// invariants resolve (F5 #28 stage 4c). Empty outside the group-op apply path.
    parents: &'a [[u8; 32]],
    /// The at-cut apply-auth decision source (F5 #28 stage 4c). The default
    /// [`LiveFallbackAuthorizer`](crate::authorizer::LiveFallbackAuthorizer) returns
    /// `None`, so the last-admin shadow is inert for non-apply constructions.
    authorizer: &'a dyn crate::authorizer::AtCutAuthorizer,
}

impl<'a> MembershipPolicy<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        let membership = GroupMembershipView::new(store, group_id);
        Self {
            store,
            group_id,
            membership,
            parents: &[],
            authorizer: &crate::authorizer::LIVE_FALLBACK_AUTHORIZER,
        }
    }

    /// Attach the op's causal cut + the at-cut authorizer so the last-admin
    /// invariants SHADOW the projection's PARENT-cut verdict against live (F5 #28
    /// stage 4c, plane `last-admin`). Without it (the default) the shadow is inert.
    #[must_use]
    pub fn with_apply_auth(
        mut self,
        parents: &'a [[u8; 32]],
        authorizer: &'a dyn crate::authorizer::AtCutAuthorizer,
    ) -> Self {
        self.parents = parents;
        self.authorizer = authorizer;
        self
    }

    /// Would removing/demoting `member` orphan `group`'s admins? Resolved at the op's
    /// PARENT cut from the projection — `is_last_admin_at_cut` reads the pre-mutation
    /// admin set as of the op's own parents, which is the correct cut for a check the
    /// op is about to invalidate.
    ///
    /// When the projection abstains, fall back to live ONLY if the abstention means
    /// "there is no cut to resolve against" (a genesis op, or a construction with no
    /// apply-auth context — the emit path, the local apply, tests). If the cut is real
    /// but unfolded, refuse: a live answer would depend on which concurrent role ops
    /// this replica has folded, so two replicas could disagree on whether the SAME op
    /// orphans the admin set — one applying it and one rejecting it forever.
    fn would_orphan_admins(&self, member: &AccountId) -> EyreResult<bool> {
        if let Some(blocks) =
            self.authorizer
                .is_last_admin_at_cut(&self.group_id, member, self.parents)
        {
            return Ok(blocks);
        }
        if !self
            .authorizer
            .can_resolve_cut(&self.group_id, self.parents)
        {
            bail!(crate::ApplyError::AuthorityUndecidable {
                group_id: format!("{:?}", self.group_id),
                signer: format!("{member}"),
            });
        }
        Ok(self.membership.is_admin(member)? && !self.membership.has_another_admin(member)?)
    }

    pub fn ensure_not_last_admin_removal(&self, member: &AccountId) -> EyreResult<()> {
        if self.would_orphan_admins(member)? {
            bail!(MembershipError::LastAdmin);
        }
        Ok(())
    }

    pub fn ensure_not_last_admin_demotion(
        &self,
        member: &AccountId,
        new_role: &GroupMemberRole,
    ) -> EyreResult<()> {
        if *new_role == GroupMemberRole::Admin {
            return Ok(());
        }
        if self.would_orphan_admins(member)? {
            bail!(MembershipError::LastAdminDemotion);
        }
        Ok(())
    }

    /// Whether `verifier` may vouch for a TEE attestation in this group: an
    /// admin of it (directly, as its genesis admin, or inherited from an
    /// ancestor), or a TEE node already admitted to it (`ReadOnlyTee`).
    ///
    /// Plain membership is NOT enough. Peers never see the quote — the op
    /// carries only the measurements the verifier claims — so whoever may sign
    /// this op decides who gets the group key. A plain `Member` must not be able
    /// to mint a "TEE" out of an arbitrary key by copying measurements the
    /// policy allows.
    ///
    /// The TEE row is read in THIS group and nowhere else: a TEE admitted to the
    /// namespace root vouches at the root, and one admitted to a Restricted
    /// subgroup vouches there. Neither can vouch in a group it was never let into.
    pub fn is_tee_attestation_verifier(&self, verifier: &AccountId) -> EyreResult<bool> {
        if MembershipRepository::new(self.store).is_inherited_admin(&self.group_id, verifier)? {
            return Ok(true);
        }
        Ok(self.membership.role_of(verifier)? == Some(GroupMemberRole::ReadOnlyTee))
    }

    /// [`is_tee_attestation_verifier`](Self::is_tee_attestation_verifier) as a
    /// gate.
    ///
    /// Takes the verifier's ACCOUNT: the caller — which holds the op's signing
    /// key — resolves it first so an unbound or revoked key refuses here rather
    /// than being promoted to a stand-in that matches nothing.
    pub fn require_tee_attestation_verifier(&self, verifier: &AccountId) -> EyreResult<()> {
        if !self.is_tee_attestation_verifier(verifier)? {
            bail!(MembershipError::TeeVerifierNotAuthorized);
        }
        Ok(())
    }

    pub fn read_required_tee_admission_policy(&self) -> EyreResult<TeeAdmissionPolicy> {
        match read_tee_admission_policy(self.store, &self.group_id)? {
            TeeAdmissionPolicyRead::Set(policy) => Ok(policy),
            TeeAdmissionPolicyRead::NotSet => bail!(MembershipError::NoTeeAdmissionPolicy),
            TeeAdmissionPolicyRead::Unreadable { undecodable } => {
                bail!(MembershipError::TeeAdmissionPolicyUnreadable(
                    undecodable.len()
                ))
            }
        }
    }

    pub fn validate_tee_attestation_allowlists(
        &self,
        policy: &TeeAdmissionPolicy,
        claims: &TeeAttestationClaims<'_>,
    ) -> EyreResult<()> {
        self.validate_tee_attestation_allowlists_record(policy, claims)
    }

    pub fn validate_tee_attestation_allowlists_record(
        &self,
        policy: &TeeAdmissionPolicy,
        fields: &TeeAttestationClaims<'_>,
    ) -> EyreResult<()> {
        // Under a signed-release policy the measurements are checked by the
        // admitter against the release file it verified; a peer replaying the
        // op has neither the quote nor the network to repeat that. It trusts
        // the voucher for them, as every peer already does for the quote
        // itself, and still holds the TCB rule, which needs only the op.
        if policy.release_trust.is_some() {
            if !tcb_status_allowed(
                &policy.allowed_tcb_statuses,
                fields.tcb_status,
                false,
                policy.accept_mock,
            ) {
                record_membership_policy_rejection(TEE_REJECT_TCB_STATUS);
                bail!(MembershipPolicyValidationError {
                    reason: MembershipPolicyRejection::TcbStatusNotAllowed,
                });
            }
            return Ok(());
        }
        let normalized_policy = TeeAllowlistPolicy {
            allowed_mrtd: policy.allowed_mrtd.clone(),
            allowed_rtmr0: policy.allowed_rtmr0.clone(),
            allowed_rtmr1: policy.allowed_rtmr1.clone(),
            allowed_rtmr2: policy.allowed_rtmr2.clone(),
            allowed_rtmr3: policy.allowed_rtmr3.clone(),
            allowed_tcb_statuses: policy.allowed_tcb_statuses.clone(),
            accept_mock: policy.accept_mock,
        };
        if let Err(err) = validate_tee_attestation_allowlists(&normalized_policy, fields) {
            let reason = match err.reason() {
                MembershipPolicyRejection::MrtdNotAllowed => TEE_REJECT_MRTD,
                MembershipPolicyRejection::MrtdAllowlistEmpty => TEE_REJECT_MRTD_EMPTY,
                MembershipPolicyRejection::TcbStatusNotAllowed => TEE_REJECT_TCB_STATUS,
                MembershipPolicyRejection::Rtmr0NotAllowed => TEE_REJECT_RTMR0,
                MembershipPolicyRejection::Rtmr1NotAllowed => TEE_REJECT_RTMR1,
                MembershipPolicyRejection::Rtmr2NotAllowed => TEE_REJECT_RTMR2,
                MembershipPolicyRejection::Rtmr3NotAllowed => TEE_REJECT_RTMR3,
                MembershipPolicyRejection::Rtmr1AllowlistEmpty => TEE_REJECT_RTMR1_EMPTY,
                MembershipPolicyRejection::Rtmr2AllowlistEmpty => TEE_REJECT_RTMR2_EMPTY,
                MembershipPolicyRejection::Rtmr3AllowlistEmpty => TEE_REJECT_RTMR3_EMPTY,
            };
            record_membership_policy_rejection(reason);
            bail!(err);
        }
        Ok(())
    }

    pub fn admit_member_if_absent(
        &self,
        member: &AccountId,
        role: &GroupMemberRole,
    ) -> EyreResult<()> {
        if !self.membership.is_member(member)? {
            MembershipRepository::new(self.store).add_member(
                &self.group_id,
                member,
                role.clone(),
            )?;
        }
        Ok(())
    }
}
