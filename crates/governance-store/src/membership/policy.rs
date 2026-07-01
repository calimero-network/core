use crate::MembershipRepository;
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::super::{read_tee_admission_policy, MembershipError, TeeAdmissionPolicy};
use super::policy_rules::{
    validate_tee_attestation_allowlists, MembershipPolicyRejection, TeeAllowlistPolicy,
    TeeAttestationClaims, TEE_REJECT_MRTD, TEE_REJECT_RTMR0, TEE_REJECT_RTMR1, TEE_REJECT_RTMR2,
    TEE_REJECT_RTMR3, TEE_REJECT_TCB_STATUS,
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
    /// PARENT cut from the projection (F5 #28 stage 4c-flip) — `is_last_admin_at_cut`
    /// reads the pre-mutation admin set as of the op's own parents (the correct cut
    /// for a check the op is about to invalidate). `None` — no apply-auth context (a
    /// local pre-check / cascade / test) OR an incomplete fold — falls back to the
    /// live `is_admin && !has_another_admin`. The live fallback retires in #29b.
    fn would_orphan_admins(&self, member: &PublicKey) -> EyreResult<bool> {
        if let Some(blocks) =
            self.authorizer
                .is_last_admin_at_cut(&self.group_id, member, self.parents)
        {
            return Ok(blocks);
        }
        Ok(self.membership.is_admin(member)? && !self.membership.has_another_admin(member)?)
    }

    pub fn ensure_not_last_admin_removal(&self, member: &PublicKey) -> EyreResult<()> {
        if self.would_orphan_admins(member)? {
            bail!(MembershipError::LastAdmin);
        }
        Ok(())
    }

    pub fn ensure_not_last_admin_demotion(
        &self,
        member: &PublicKey,
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

    pub fn require_tee_attestation_verifier_membership(
        &self,
        signer: &PublicKey,
    ) -> EyreResult<()> {
        if !self.membership.is_member(signer)? {
            bail!(MembershipError::TeeVerifierNotMember);
        }
        Ok(())
    }

    pub fn read_required_tee_admission_policy(&self) -> EyreResult<TeeAdmissionPolicy> {
        read_tee_admission_policy(self.store, &self.group_id)?
            .ok_or_else(|| MembershipError::NoTeeAdmissionPolicy.into())
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
                MembershipPolicyRejection::TcbStatusNotAllowed => TEE_REJECT_TCB_STATUS,
                MembershipPolicyRejection::Rtmr0NotAllowed => TEE_REJECT_RTMR0,
                MembershipPolicyRejection::Rtmr1NotAllowed => TEE_REJECT_RTMR1,
                MembershipPolicyRejection::Rtmr2NotAllowed => TEE_REJECT_RTMR2,
                MembershipPolicyRejection::Rtmr3NotAllowed => TEE_REJECT_RTMR3,
            };
            record_membership_policy_rejection(reason);
            bail!(err);
        }
        Ok(())
    }

    pub fn admit_member_if_absent(
        &self,
        member: &PublicKey,
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
