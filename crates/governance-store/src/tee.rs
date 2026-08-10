use crate::NamespaceRepository;
use calimero_account::AccountId;
use calimero_context_client::local_governance::{
    GroupOp, NamespaceOp, RootOp, SignedGroupOp, SignedNamespaceOp,
};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::GroupMemberRole;
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
/// [`is_quote_hash_used`] and matches on the joined member's account.
pub fn is_tee_admitted_identity(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &AccountId,
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
///
/// Returns the **latest** matching op, not the first: after a removal and
/// re-admission an identity has multiple join ops, and the most recent one is
/// the live verdict (e.g. a newer TCB/measurement set). This mirrors
/// [`read_tee_admission_policy`], which also takes the last write. Reusing a
/// stale earlier verdict for subgroup fan-out would re-admit against outdated
/// attestation data.
/// The admission verdict carried by an encrypted [`GroupOp`] log entry.
///
/// Needs no group id: the per-group op log this reads is already scoped to one
/// group. Its cleartext counterpart is [`tee_admission_from_root_op`], which
/// does need one, because a namespace log carries every group's root ops.
fn tee_admission_from_bytes(bytes: &[u8]) -> Option<(AccountId, TeeAdmissionRecord)> {
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
            return Some((
                member,
                TeeAdmissionRecord {
                    quote_hash,
                    mrtd,
                    rtmr0,
                    rtmr1,
                    rtmr2,
                    rtmr3,
                    tcb_status,
                    role,
                },
            ));
        }
        return None;
    }
    None
}

/// This namespace's cleartext root ops, for the namespace owning `group_id`.
fn root_ops_for(store: &Store, group_id: &ContextGroupId) -> EyreResult<Vec<SignedNamespaceOp>> {
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;
    crate::NamespaceOpLogService::new(store, namespace.to_bytes().into()).collect_root_ops()
}

/// The admission verdict a cleartext [`RootOp`] carries, if it is one and it
/// names `group_id`.
fn tee_admission_from_root_op(
    signed: &SignedNamespaceOp,
    group_id: &ContextGroupId,
) -> Option<(AccountId, TeeAdmissionRecord)> {
    {
        if let NamespaceOp::Root(RootOp::MemberJoinedViaTeeAttestation {
            group_id: op_group,
            member: _,
            quote_hash,
            mrtd,
            rtmr0,
            rtmr1,
            rtmr2,
            rtmr3,
            tcb_status,
            role,
            account,
            ..
        }) = &signed.op
        {
            // The cleartext form names its group explicitly, so an admission
            // into a DIFFERENT group in this namespace must not be read as one
            // into `group_id`.
            if *op_group != *group_id {
                return None;
            }
            // The cleartext form names the attested KEY, while the verdict is
            // recorded against the account that admission created. Both are on
            // the op: the account comes from the credential, which the apply
            // refused unless it certifies this very key.
            return Some((
                account.cert.account,
                TeeAdmissionRecord {
                    quote_hash: *quote_hash,
                    mrtd: mrtd.clone(),
                    rtmr0: rtmr0.clone(),
                    rtmr1: rtmr1.clone(),
                    rtmr2: rtmr2.clone(),
                    rtmr3: rtmr3.clone(),
                    tcb_status: tcb_status.clone(),
                    role: role.clone(),
                },
            ));
        }
    }
    None
}

pub fn tee_admission_record(
    store: &Store,
    group_id: &ContextGroupId,
    identity: &AccountId,
) -> EyreResult<Option<TeeAdmissionRecord>> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;
    let mut latest = None;

    for (_seq, bytes) in &entries {
        if let Some((member, record)) = tee_admission_from_bytes(bytes) {
            if member == *identity {
                latest = Some(record);
            }
        }
    }

    // ...and the namespace log, where the CLEARTEXT admission lives. A fleet
    // replica's admission is a `RootOp`, so it never appears in the per-group
    // log scanned above; without this the subgroup fan-in finds no verdict for
    // a replica the namespace has plainly admitted.
    for signed in root_ops_for(store, group_id)? {
        if let Some((member, record)) = tee_admission_from_root_op(&signed, group_id) {
            if member == *identity {
                latest = Some(record);
            }
        }
    }

    Ok(latest)
}

/// Like [`tee_admission_record`] but returns the verdict for *every* TEE
/// member admitted into `group_id`, from a SINGLE op-log scan.
///
/// A caller that needs many members' verdicts (e.g. admitting all root TEE
/// members into a newly-created subgroup) would otherwise call
/// [`tee_admission_record`] once per member, re-scanning the same op log each
/// time — O(members × log). This folds them in one pass. Last-write-wins per
/// member, matching [`tee_admission_record`]'s "latest matching op" semantics
/// (a re-admission after removal supersedes the earlier verdict).
pub fn tee_admission_records(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<std::collections::BTreeMap<AccountId, TeeAdmissionRecord>> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;
    let mut out = std::collections::BTreeMap::new();

    for (_seq, bytes) in &entries {
        if let Some((member, record)) = tee_admission_from_bytes(bytes) {
            // Last-write-wins: a later re-admission supersedes the earlier verdict.
            let _ = out.insert(member, record);
        }
    }

    for signed in root_ops_for(store, group_id)? {
        if let Some((member, record)) = tee_admission_from_root_op(&signed, group_id) {
            let _ = out.insert(member, record);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_context_client::local_governance::{GroupOp, SignedGroupOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;

    use super::{tee_admission_record, tee_admission_records};
    use crate::local_state::append_op_log_entry;
    use crate::test_fixtures::test_store;

    fn tee_join_op(
        signer_sk: &PrivateKey,
        ns_gid: ContextGroupId,
        nonce: u64,
        member: AccountId,
        quote_hash: [u8; 32],
    ) -> SignedGroupOp {
        SignedGroupOp::sign(
            signer_sk,
            ns_gid,
            vec![],
            nonce,
            GroupOp::MemberJoinedViaTeeAttestation {
                member,
                quote_hash,
                mrtd: "m1".to_owned(),
                rtmr0: "r0".to_owned(),
                rtmr1: "r1".to_owned(),
                rtmr2: "r2".to_owned(),
                rtmr3: "r3".to_owned(),
                tcb_status: "UpToDate".to_owned(),
                role: GroupMemberRole::ReadOnlyTee,
            },
        )
        .unwrap()
    }

    #[test]
    fn record_returns_stored_verdict_for_admitted_member() {
        let store = test_store();
        let mut rng = rand::thread_rng();
        let namespace_id = [0xAA; 32];
        let ns_gid = ContextGroupId::from(namespace_id);
        let tee_pk = AccountId::from([0x42; 32]);
        let unknown = AccountId::from([0x43; 32]);

        let signer_sk = PrivateKey::random(&mut rng);
        let tee_op = SignedGroupOp::sign(
            &signer_sk,
            ns_gid,
            vec![],
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

    #[test]
    fn records_returns_all_verdicts_in_one_scan() {
        let store = test_store();
        let mut rng = rand::thread_rng();
        let ns_gid = ContextGroupId::from([0xAA; 32]);
        let tee_a = AccountId::from([0x42; 32]);
        let tee_b = AccountId::from([0x44; 32]);
        let signer_sk = PrivateKey::random(&mut rng);

        append_op_log_entry(
            &store,
            &ns_gid,
            1,
            &borsh::to_vec(&tee_join_op(&signer_sk, ns_gid, 1, tee_a, [0x07; 32])).unwrap(),
        )
        .unwrap();
        append_op_log_entry(
            &store,
            &ns_gid,
            2,
            &borsh::to_vec(&tee_join_op(&signer_sk, ns_gid, 2, tee_b, [0x08; 32])).unwrap(),
        )
        .unwrap();
        // A re-admission op for tee_a (e.g. after removal + re-admit) — the
        // latest verdict supersedes the earlier one.
        append_op_log_entry(
            &store,
            &ns_gid,
            3,
            &borsh::to_vec(&tee_join_op(&signer_sk, ns_gid, 3, tee_a, [0x09; 32])).unwrap(),
        )
        .unwrap();

        let records = tee_admission_records(&store, &ns_gid).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[&tee_a].quote_hash, [0x09; 32], "last-write-wins");
        assert_eq!(records[&tee_b].quote_hash, [0x08; 32]);

        // Each entry matches what the single-member read returns (also latest).
        for member in [tee_a, tee_b] {
            assert_eq!(
                tee_admission_record(&store, &ns_gid, &member)
                    .unwrap()
                    .unwrap()
                    .quote_hash,
                records[&member].quote_hash
            );
        }
    }

    #[test]
    fn record_returns_latest_verdict_after_readmit() {
        let store = test_store();
        let mut rng = rand::thread_rng();
        let ns_gid = ContextGroupId::from([0xAB; 32]);
        let tee_pk = AccountId::from([0x42; 32]);
        let signer_sk = PrivateKey::random(&mut rng);

        // Original admission, then a later re-admission with a fresh quote.
        append_op_log_entry(
            &store,
            &ns_gid,
            1,
            &borsh::to_vec(&tee_join_op(&signer_sk, ns_gid, 1, tee_pk, [0x07; 32])).unwrap(),
        )
        .unwrap();
        append_op_log_entry(
            &store,
            &ns_gid,
            2,
            &borsh::to_vec(&tee_join_op(&signer_sk, ns_gid, 2, tee_pk, [0x09; 32])).unwrap(),
        )
        .unwrap();

        assert_eq!(
            tee_admission_record(&store, &ns_gid, &tee_pk)
                .unwrap()
                .unwrap()
                .quote_hash,
            [0x09; 32],
            "must reuse the most recent admission verdict, not the stale first one"
        );
    }
}
