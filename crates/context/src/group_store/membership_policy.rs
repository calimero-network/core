use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{
    add_group_member,
    membership_policy_rules::{
        validate_tee_attestation_allowlists, MembershipPolicyRejection, TeeAllowlistPolicy,
        TeeAttestationClaims, TEE_REJECT_MRTD, TEE_REJECT_RTMR0, TEE_REJECT_RTMR1,
        TEE_REJECT_RTMR2, TEE_REJECT_RTMR3, TEE_REJECT_TCB_STATUS,
    },
    membership_view::GroupMembershipView,
    read_tee_admission_policy, GroupStoreError, TeeAdmissionPolicy,
};
use crate::metrics::record_membership_policy_rejection;

/// Membership policy service for governance mutations.
///
/// Encapsulates business rules around admin cardinality and TEE admission
/// allowlists so mutation handlers can stay focused on state transitions.
pub struct MembershipPolicy<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
    membership: GroupMembershipView<'a>,
}

impl<'a> MembershipPolicy<'a> {
    pub fn new(store: &'a Store, group_id: ContextGroupId) -> Self {
        let membership = GroupMembershipView::new(store, group_id);
        Self {
            store,
            group_id,
            membership,
        }
    }

    pub fn ensure_not_last_admin_removal(&self, member: &PublicKey) -> EyreResult<()> {
        if !self.membership.is_admin(member)? {
            return Ok(());
        }
        if self.membership.has_another_admin(member)? {
            return Ok(());
        }
        bail!(GroupStoreError::LastAdmin);
    }

    pub fn ensure_not_last_admin_demotion(
        &self,
        member: &PublicKey,
        new_role: &GroupMemberRole,
    ) -> EyreResult<()> {
        if *new_role == GroupMemberRole::Admin {
            return Ok(());
        }
        if !self.membership.is_admin(member)? {
            return Ok(());
        }
        if self.membership.has_another_admin(member)? {
            return Ok(());
        }
        bail!(GroupStoreError::LastAdminDemotion);
    }

    pub fn require_tee_attestation_verifier_membership(
        &self,
        signer: &PublicKey,
    ) -> EyreResult<()> {
        if !self.membership.is_member(signer)? {
            bail!("TEE attestation verifier must be a group member");
        }
        Ok(())
    }

    pub fn read_required_tee_admission_policy(&self) -> EyreResult<TeeAdmissionPolicy> {
        read_tee_admission_policy(self.store, &self.group_id)?.ok_or_else(|| {
            eyre::eyre!(
                "MemberJoinedViaTeeAttestation rejected: no TeeAdmissionPolicySet exists for group"
            )
        })
    }

    pub fn validate_tee_attestation_allowlists(
        &self,
        policy: &TeeAdmissionPolicy,
        mrtd: &str,
        rtmr0: &str,
        rtmr1: &str,
        rtmr2: &str,
        rtmr3: &str,
        tcb_status: &str,
    ) -> EyreResult<()> {
        self.validate_tee_attestation_allowlists_record(
            policy,
            &TeeAttestationClaims {
                mrtd,
                rtmr0,
                rtmr1,
                rtmr2,
                rtmr3,
                tcb_status,
            },
        )
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
            add_group_member(self.store, &self.group_id, member, role.clone())?;
        }
        Ok(())
    }
}
