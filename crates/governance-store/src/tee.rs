use crate::NamespaceRepository;
use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
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

/// The verified TEE admission verdict read back from a
/// `MemberJoinedViaTeeAttestation` op in a group's governance log.
///
/// Mirrors the fields recorded by [`is_tee_admitted_identity`]'s op, but
/// returns the stored attestation measurements and role instead of a bool.
/// Used to reuse a verdict already verified at namespace-root admission
/// when transparently re-admitting the same TEE node into a subgroup.
#[derive(Clone, Debug)]
pub struct TeeAdmissionRecord {
    pub quote_hash: [u8; 32],
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
    pub tcb_status: String,
    pub role: GroupMemberRole,
}

/// Return the stored TEE admission verdict for `identity` in `group_id`, if
/// the identity joined via a `MemberJoinedViaTeeAttestation` op. Scans the
/// same op log as [`is_tee_admitted_identity`] but destructures all recorded
/// fields. Returns `None` for an unknown member.
pub fn tee_admission_record(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &PublicKey,
) -> EyreResult<Option<TeeAdmissionRecord>> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;

    for (_seq, bytes) in &entries {
        if let Ok(op) = borsh::from_slice::<SignedGroupOp>(bytes) {
            if let GroupOp::MemberJoinedViaTeeAttestation {
                member,
                quote_hash,
                mrtd,
                rtmr0,
                rtmr1,
                rtmr2,
                rtmr3,
                tcb_status,
                role,
            } = op.op
            {
                if member == *identity {
                    return Ok(Some(TeeAdmissionRecord {
                        quote_hash,
                        mrtd,
                        rtmr0,
                        rtmr1,
                        rtmr2,
                        rtmr3,
                        tcb_status,
                        role,
                    }));
                }
            }
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::{PrivateKey, PublicKey};

    use super::tee_admission_record;
    use crate::local_state::append_op_log_entry;
    use crate::test_fixtures::test_store;

    #[test]
    fn record_returns_stored_verdict_for_admitted_member() {
        let store = test_store();
        let mut rng = rand::thread_rng();
        let namespace_id = [0xAA; 32];
        let ns_gid = ContextGroupId::from(namespace_id);
        let tee_pk = PublicKey::from([0x42; 32]);
        let unknown = PublicKey::from([0x43; 32]);

        let signer_sk = PrivateKey::random(&mut rng);
        let tee_op = SignedGroupOp::sign(
            &signer_sk,
            ns_gid.to_bytes(),
            vec![],
            [0u8; 32],
            1,
            GroupOp::MemberJoinedViaTeeAttestation {
                member: tee_pk,
                quote_hash: [0x07; 32],
                mrtd: "m1".to_owned(),
                rtmr0: "r0".to_owned(),
                rtmr1: "r1".to_owned(),
                rtmr2: "r2".to_owned(),
                rtmr3: "r3".to_owned(),
                tcb_status: "UpToDate".to_owned(),
                role: GroupMemberRole::ReadOnlyTee,
            },
        )
        .unwrap();
        append_op_log_entry(&store, &ns_gid, 1, &borsh::to_vec(&tee_op).unwrap()).unwrap();

        let record = tee_admission_record(&store, &ns_gid, &tee_pk)
            .unwrap()
            .expect("admitted member must have a record");
        assert_eq!(record.quote_hash, [0x07; 32]);
        assert_eq!(record.mrtd, "m1");
        assert_eq!(record.rtmr0, "r0");
        assert_eq!(record.rtmr1, "r1");
        assert_eq!(record.rtmr2, "r2");
        assert_eq!(record.rtmr3, "r3");
        assert_eq!(record.tcb_status, "UpToDate");
        assert_eq!(record.role, GroupMemberRole::ReadOnlyTee);

        assert!(tee_admission_record(&store, &ns_gid, &unknown)
            .unwrap()
            .is_none());
    }
}
