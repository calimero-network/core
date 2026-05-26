use crate::group_store::NamespaceRepository;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::read_op_log_after;

/// Reconstructed TEE admission policy from the governance DAG.
#[derive(Debug)]
pub struct TeeAdmissionPolicy {
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
    pub accept_mock: bool,
}

/// Read the TEE admission policy that applies to `group_id`.
///
/// Policies are namespace-scoped: the canonical policy lives on the namespace
/// root's governance op log. Subgroups resolve to their root before reading,
/// so any policy bytes that may exist on a subgroup's own log are intentionally
/// ignored. See `project_subgroup_policy_decision.md` for the design rationale;
/// auto-follow already propagates membership across subgroups without a second
/// admission check, so per-subgroup policies were inert.
pub fn read_tee_admission_policy(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Option<TeeAdmissionPolicy>> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let entries = read_op_log_after(store, &root, 0, usize::MAX)?;
    let mut latest: Option<TeeAdmissionPolicy> = None;

    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::TeeAdmissionPolicySet {
                allowed_mrtd,
                allowed_rtmr0,
                allowed_rtmr1,
                allowed_rtmr2,
                allowed_rtmr3,
                allowed_tcb_statuses,
                accept_mock,
            } = op.op
            {
                latest = Some(TeeAdmissionPolicy {
                    allowed_mrtd,
                    allowed_rtmr0,
                    allowed_rtmr1,
                    allowed_rtmr2,
                    allowed_rtmr3,
                    allowed_tcb_statuses,
                    accept_mock,
                });
            }
        }
    }

    Ok(latest)
}

/// Check whether a TEE attestation quote hash has already been used in a
/// `MemberJoinedViaTeeAttestation` op for this group.
pub fn is_quote_hash_used(
    store: &Store,
    group_id: &ContextGroupId,
    quote_hash: &[u8; 32],
) -> EyreResult<bool> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;

    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::MemberJoinedViaTeeAttestation {
                quote_hash: ref existing_hash,
                ..
            } = op.op
            {
                if existing_hash == quote_hash {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

/// True if `identity` joined `group_id` via a `MemberJoinedViaTeeAttestation`
/// op. TEE nodes have no separate roster — admission is recorded only by
/// that op in the governance log, so this scans the same op log as
/// [`is_quote_hash_used`] and matches on the joined member's identity.
pub fn is_tee_admitted_identity(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<bool> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;

    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::MemberJoinedViaTeeAttestation { member, .. } = op.op {
                if member == *identity {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}
