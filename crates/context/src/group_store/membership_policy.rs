use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::{bail, Result as EyreResult};

use super::{
    add_group_member, membership_view::GroupMembershipView, read_tee_admission_policy,
    GroupStoreError, TeeAdmissionPolicy,
};

/// Membership policy service for governance mutations.
///
/// Encapsulates business rules around admin cardinality and TEE admission
/// allowlists so mutation handlers can stay focused on state transitions.
pub struct MembershipPolicy<'a> {
    store: &'a Store,
    group_id: ContextGroupId,
    membership: GroupMembershipView<'a>,
}

pub struct TeeAttestationFields<'a> {
    pub mrtd: &'a str,
    pub rtmr0: &'a str,
    pub rtmr1: &'a str,
    pub rtmr2: &'a str,
    pub rtmr3: &'a str,
    pub tcb_status: &'a str,
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
            &TeeAttestationFields {
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
        fields: &TeeAttestationFields<'_>,
    ) -> EyreResult<()> {
        // Empty allowlists are intentionally permissive (allow-all for that field).
        if !policy.allowed_mrtd.is_empty() && !policy.allowed_mrtd.iter().any(|a| a == fields.mrtd)
        {
            bail!("MemberJoinedViaTeeAttestation rejected: MRTD not in policy allowlist");
        }
        if !policy.allowed_tcb_statuses.is_empty()
            && !policy
                .allowed_tcb_statuses
                .iter()
                .any(|a| a == fields.tcb_status)
        {
            bail!("MemberJoinedViaTeeAttestation rejected: TCB status not in policy allowlist");
        }
        for (allowlist, actual, label) in [
            (&policy.allowed_rtmr0, fields.rtmr0, "RTMR0"),
            (&policy.allowed_rtmr1, fields.rtmr1, "RTMR1"),
            (&policy.allowed_rtmr2, fields.rtmr2, "RTMR2"),
            (&policy.allowed_rtmr3, fields.rtmr3, "RTMR3"),
        ] {
            if !allowlist.is_empty() && !allowlist.iter().any(|a| a == actual) {
                bail!("MemberJoinedViaTeeAttestation rejected: {label} not in policy allowlist");
            }
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
