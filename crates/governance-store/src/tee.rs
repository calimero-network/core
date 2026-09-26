use crate::{MembershipPath, MembershipRepository, NamespaceRepository};
use calimero_account::AccountId;
use calimero_context_client::local_governance::{GroupOp, NamespaceOp, RootOp, SignedGroupOp};
use calimero_context_config::types::ContextGroupId;
use calimero_primitives::context::{ContextId, GroupMemberRole};
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result as EyreResult;

use super::read_op_log_after;

/// Reconstructed TEE admission policy from the governance DAG.
///
/// Two forms share this struct. A `TeeAdmissionPolicySet` fills the measurement
/// lists and leaves `release_trust` empty. A `TeeReleaseAdmissionPolicySet`
/// sets `release_trust` and leaves the lists empty: the measurements come from
/// the signed release the TEE runs, checked by the admitter, not from here.
#[derive(Debug)]
pub struct TeeAdmissionPolicy {
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
    pub accept_mock: bool,
    pub release_trust: Option<TeeReleaseTrust>,
}

/// The signed-release half of a [`TeeAdmissionPolicy`].
#[derive(Clone, Debug)]
pub struct TeeReleaseTrust {
    /// Image profiles a TEE may run, e.g. `locked-read-only`.
    pub allowed_profiles: Vec<String>,
    /// The oldest release admitted, or `None` for any signed release.
    pub min_release_version: Option<String>,
}

/// One op-log entry that could not be decoded as a [`SignedGroupOp`].
#[derive(Clone, Debug)]
pub struct UndecodableOpLogEntry {
    pub sequence: u64,
    pub error: String,
}

/// What replaying a namespace's op log found when looking for its TEE
/// admission policy.
///
/// Three states rather than an `Option`, because "no policy was ever set" and
/// "a policy may be set but the log cannot be read" are opposite situations
/// that used to collapse into the same `None`. The policy is not a
/// materialized row -- it exists only as this replay -- so a single entry that
/// does not decode does not degrade the answer, it can *erase* it. An operator
/// who has set a policy would then be told to set one.
#[derive(Debug)]
pub enum TeeAdmissionPolicyRead {
    /// A `TeeAdmissionPolicySet` was found. Note that entries which failed to
    /// decode are logged even in this arm: one of them may have carried a
    /// *later* policy that supersedes this one.
    Set(TeeAdmissionPolicy),
    /// Every entry decoded and none of them was a policy. The group genuinely
    /// has no policy set.
    NotSet,
    /// No policy was found among the entries that decode, and at least one
    /// entry does not decode. Whether a policy is set is unknown, so callers
    /// must not report this as "no policy set".
    Unreadable {
        undecodable: Vec<UndecodableOpLogEntry>,
    },
}

/// Decode one op-log entry, reporting a failure instead of dropping it.
///
/// Every scan in this module replays the same log with `if let Ok(op)`. An
/// entry that does not decode is not "an entry that is not the op I want" --
/// it is an entry that nobody can read, and the difference matters: a
/// truncated write, a schema change or a `SignedGroupOp` version skew was
/// indistinguishable from an absent op. `scan` names the caller so the log
/// says which read was affected.
fn decode_group_op(
    group_id: &ContextGroupId,
    sequence: u64,
    bytes: &[u8],
    scan: &'static str,
) -> Result<SignedGroupOp, String> {
    borsh::from_slice::<SignedGroupOp>(bytes).map_err(|e| {
        let error = e.to_string();
        tracing::warn!(
            group_id = %hex::encode(group_id.to_bytes()),
            sequence,
            scan,
            error = %error,
            "op-log entry does not decode as SignedGroupOp; it is skipped, so any op it \
             carried is invisible to this scan"
        );
        error
    })
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
) -> EyreResult<TeeAdmissionPolicyRead> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let entries = read_op_log_after(store, &root, 0, usize::MAX)?;
    let mut latest: Option<TeeAdmissionPolicy> = None;
    let mut undecodable: Vec<UndecodableOpLogEntry> = Vec::new();

    for (seq, bytes) in &entries {
        let op = match decode_group_op(&root, *seq, bytes, "read_tee_admission_policy") {
            Ok(op) => op,
            Err(error) => {
                undecodable.push(UndecodableOpLogEntry {
                    sequence: *seq,
                    error,
                });
                continue;
            }
        };

        // Either form supersedes the other: the newest policy op in the log
        // is the policy, whichever kind it is.
        match op.op {
            GroupOp::TeeAdmissionPolicySet {
                allowed_mrtd,
                allowed_rtmr0,
                allowed_rtmr1,
                allowed_rtmr2,
                allowed_rtmr3,
                allowed_tcb_statuses,
                accept_mock,
            } => {
                latest = Some(TeeAdmissionPolicy {
                    allowed_mrtd,
                    allowed_rtmr0,
                    allowed_rtmr1,
                    allowed_rtmr2,
                    allowed_rtmr3,
                    allowed_tcb_statuses,
                    accept_mock,
                    release_trust: None,
                });
            }
            GroupOp::TeeReleaseAdmissionPolicySet {
                allowed_profiles,
                min_release_version,
                allowed_tcb_statuses,
                accept_mock,
            } => {
                latest = Some(TeeAdmissionPolicy {
                    allowed_mrtd: Vec::new(),
                    allowed_rtmr0: Vec::new(),
                    allowed_rtmr1: Vec::new(),
                    allowed_rtmr2: Vec::new(),
                    allowed_rtmr3: Vec::new(),
                    allowed_tcb_statuses,
                    accept_mock,
                    release_trust: Some(TeeReleaseTrust {
                        allowed_profiles,
                        min_release_version,
                    }),
                });
            }
            _ => {}
        }
    }

    match latest {
        // A readable policy still stands: the entries that failed to decode
        // carried *something*, and refusing admission over them would take a
        // working namespace down for an unrelated bad write. But one of them
        // may have been a LATER policy that supersedes this one, so the
        // policy being enforced is not provably current -- say so.
        Some(policy) => {
            if !undecodable.is_empty() {
                tracing::warn!(
                    group_id = %hex::encode(root.to_bytes()),
                    undecodable = undecodable.len(),
                    "enforcing the newest TEE admission policy that decodes, but some op-log \
                     entries do not decode; if one of those was a later policy, this one is \
                     stale"
                );
            }
            Ok(TeeAdmissionPolicyRead::Set(policy))
        }
        // The distinction this whole type exists for. Reporting these the
        // same way tells an operator who HAS set a policy to go set one.
        None if !undecodable.is_empty() => {
            tracing::error!(
                group_id = %hex::encode(root.to_bytes()),
                undecodable = undecodable.len(),
                "no TEE admission policy could be read and some op-log entries do not decode; \
                 this is NOT the same as no policy being set"
            );
            Ok(TeeAdmissionPolicyRead::Unreadable { undecodable })
        }
        None => Ok(TeeAdmissionPolicyRead::NotSet),
    }
}

/// Read the TEE authoring policy that applies to `group_id`: the MRTDs whose
/// admitted TEEs may author as [`AccountId::TEE_AUTHORITY`].
///
/// Namespace-scoped like the admission policy: resolves to the root and takes
/// the **last** `TeeAuthoringPolicySet` on its log. No op, or an op with an empty
/// list, both read as an empty allowlist, which every caller treats as "TEE
/// authorship is off" — this policy fails closed.
pub fn read_tee_authoring_policy(
    store: &Store,
    group_id: &ContextGroupId,
) -> EyreResult<Vec<String>> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let mut allowed = Vec::new();
    for (seq, bytes) in &read_op_log_after(store, &root, 0, usize::MAX)? {
        let Ok(op) = decode_group_op(&root, *seq, bytes, "read_tee_authoring_policy") else {
            continue;
        };
        if let GroupOp::TeeAuthoringPolicySet { allowed_mrtd } = op.op {
            allowed = allowed_mrtd;
        }
    }
    Ok(allowed)
}

/// Whether `account` is a **TEE authority** for `group_id`: a TEE admitted to
/// the namespace by attestation (a direct `ReadOnlyTee` row at the root), still
/// a member of `group_id`, holding verified attestation evidence whose MRTD the
/// namespace's authoring policy allows.
///
/// The MRTD comes from the quote in a [`GroupOp::TeeAuthorityEvidence`] that
/// this node verified itself, never from an admission op's claims. An
/// admission op carries only its signer's word for the measurements, and any
/// member may sign one.
///
/// Evaluated against this node's current governance state, not at a delta's
/// causal cut. Two peers that have folded a policy change to different depths
/// can therefore briefly disagree about a TEE write near that change; see the
/// TEE authorship design notes for the at-cut follow-up.
pub fn is_tee_authority(
    store: &Store,
    group_id: &ContextGroupId,
    account: &AccountId,
) -> EyreResult<bool> {
    Ok(tee_authority_key(store, group_id, account)?.is_some())
}

/// The one key that may act as the TEE authority for `account`, or `None` if
/// `account` is not a TEE authority. It is the key the verified quote binds.
///
/// # Errors
/// Any governance store read error.
pub fn tee_authority_key(
    store: &Store,
    group_id: &ContextGroupId,
    account: &AccountId,
) -> EyreResult<Option<PublicKey>> {
    // A point lookup first: this runs for every signer the receive path
    // resolves, and nearly every one is an ordinary member. Only a direct
    // `ReadOnlyTee` row at the root — which attestation admission alone mints,
    // and removal deletes — earns the op-log scans below.
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let membership = MembershipRepository::new(store);
    if membership.role_of(&root, account)? != Some(GroupMemberRole::ReadOnlyTee) {
        return Ok(None);
    }
    // Still a member where it writes: a Restricted subgroup it was never
    // admitted to is not one.
    if membership.check_path(group_id, account)? == MembershipPath::None {
        return Ok(None);
    }
    let allowed = read_tee_authoring_policy(store, group_id)?;
    if allowed.is_empty() {
        return Ok(None);
    }
    if tee_admission_record(store, &root, account)?
        .is_none_or(|record| record.role != GroupMemberRole::ReadOnlyTee)
    {
        return Ok(None);
    }
    Ok(tee_authority_evidence(store, &root, account)?
        .filter(|evidence| allowed.contains(&evidence.mrtd))
        .map(|evidence| evidence.attested_key))
}

/// What a TEE's verified attestation evidence established.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TeeAuthorityEvidenceRecord {
    /// The key the quote binds.
    pub attested_key: PublicKey,
    /// The MRTD read from the verified quote.
    pub mrtd: String,
}

/// The latest verified [`GroupOp::TeeAuthorityEvidence`] for `account` on the
/// namespace root's log.
///
/// Each candidate is verified again here, rather than trusted because it was
/// logged: the check is pure, so it costs a signature verification, and it
/// keeps the answer right even for a log that was filled by some path other
/// than apply. The evidence must also bind a key that speaks for `account`, so
/// evidence copied from another TEE and relabelled is ignored.
///
/// # Errors
/// Any governance store read error.
pub fn tee_authority_evidence(
    store: &Store,
    group_id: &ContextGroupId,
    account: &AccountId,
) -> EyreResult<Option<TeeAuthorityEvidenceRecord>> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    let mut latest = None;
    for (seq, bytes) in &read_op_log_after(store, &root, 0, usize::MAX)? {
        let Ok(op) = decode_group_op(&root, *seq, bytes, "tee_authority_evidence") else {
            continue;
        };
        let GroupOp::TeeAuthorityEvidence {
            member,
            attested_key,
            quote,
            collateral,
            attested_at,
        } = op.op
        else {
            continue;
        };
        if member != *account {
            continue;
        }
        let Ok(verdict) =
            verify_authority_evidence(&attested_key, &quote, collateral.as_deref(), attested_at)
        else {
            continue;
        };
        if crate::member_account_in_namespace(store, &root, &attested_key)? != Some(member) {
            continue;
        }
        latest = Some(TeeAuthorityEvidenceRecord {
            attested_key,
            mrtd: verdict.mrtd,
        });
    }
    Ok(latest)
}

/// Whether `account` is owed a [`GroupOp::TeeAuthorityEvidence`] it does not
/// have yet: it was admitted to the namespace as a TEE (a direct `ReadOnlyTee`
/// row at the root), TEE authorship is on there, and no verified evidence for
/// it is on the log.
///
/// Only the TEE holds the quote the evidence carries, so only the TEE can make
/// this right, by announcing itself again: the admitting side publishes the
/// evidence for an already-admitted TEE that has none. Without that, one failed
/// publish right after admission would leave the TEE unable to author for good,
/// because it stops announcing once admitted.
///
/// Whether the policy names the TEE's MRTD is deliberately not part of this.
/// Evidence is owed either way, and asking that would re-announce forever on a
/// policy that simply does not list this image.
///
/// # Errors
/// Any governance store read error.
pub fn tee_evidence_owed(
    store: &Store,
    group_id: &ContextGroupId,
    account: &AccountId,
) -> EyreResult<bool> {
    let root = NamespaceRepository::new(store).resolve(group_id)?;
    if MembershipRepository::new(store).role_of(&root, account)?
        != Some(GroupMemberRole::ReadOnlyTee)
    {
        return Ok(false);
    }
    if read_tee_authoring_policy(store, &root)?.is_empty() {
        return Ok(false);
    }
    Ok(tee_authority_evidence(store, &root, account)?.is_none())
}

/// Verify TEE authority evidence offline. The quote must bind `attested_key` the
/// way fleet join binds it: SHA-256 of the key in report data bytes `32..64`.
///
/// # Errors
/// If the collateral does not decode, or the evidence does not verify.
pub(crate) fn verify_authority_evidence(
    attested_key: &PublicKey,
    quote: &[u8],
    collateral: Option<&[u8]>,
    attested_at: u64,
) -> EyreResult<calimero_tee_attestation::EvidenceVerdict> {
    use sha2::{Digest, Sha256};

    let collateral = collateral
        .map(serde_json::from_slice::<calimero_tee_attestation::QuoteCollateralV3>)
        .transpose()
        .map_err(|err| eyre::eyre!("TEE evidence collateral does not decode: {err}"))?;
    let key_hash: [u8; 32] = Sha256::digest(**attested_key).into();
    calimero_tee_attestation::verify_evidence(quote, collateral.as_ref(), attested_at, &key_hash)
        .map_err(|err| eyre::eyre!("TEE evidence does not verify: {err}"))
}

/// The account a write signed by `key`, which speaks for `account`, is checked
/// against at merge.
///
/// [`AccountId::TEE_AUTHORITY`] only when `account` is a TEE authority of
/// `group_id` and `key` is the key its verified quote binds (see
/// [`tee_authority_key`]), so its writes match a `TeeOnly` cell's writer set.
/// `account` itself for everyone else, so no other signer can ever match it.
/// Every path that resolves a signer for the merge must go through this, or a
/// path that skips it refuses the TEE's writes.
///
/// # Errors
/// Any governance store read error. A caller must refuse the write on an
/// error, not fall back to `account`.
pub fn writer_account(
    store: &Store,
    group_id: &ContextGroupId,
    key: &PublicKey,
    account: AccountId,
) -> EyreResult<AccountId> {
    Ok(
        if tee_authority_key(store, group_id, &account)?.as_ref() == Some(key) {
            AccountId::TEE_AUTHORITY
        } else {
            account
        },
    )
}

/// [`is_tee_authority`] for the device key that signed a delta in `context_id`.
///
/// `false` for a context owned by no group, and for a key bound to no account in
/// the namespace: neither can name an attested TEE.
pub fn is_tee_authority_for_context(
    store: &Store,
    context_id: &ContextId,
    author: &PublicKey,
) -> EyreResult<bool> {
    let Some(group_id) = crate::get_group_for_context(store, context_id)? else {
        return Ok(false);
    };
    let Some(account) = crate::member_account_in_namespace(store, &group_id, author)? else {
        return Ok(false);
    };
    Ok(tee_authority_key(store, &group_id, &account)?.as_ref() == Some(author))
}

/// Every TEE authority for `context_id`, in account order. The TEE scheduler
/// ranks these to decide which one fires a trigger.
pub fn tee_authorities_for_context(
    store: &Store,
    context_id: &ContextId,
) -> EyreResult<Vec<AccountId>> {
    let Some(group_id) = crate::get_group_for_context(store, context_id)? else {
        return Ok(Vec::new());
    };
    let root = NamespaceRepository::new(store).resolve(&group_id)?;
    let mut authorities = Vec::new();
    for account in tee_admission_records(store, &root)?.into_keys() {
        if is_tee_authority(store, &group_id, &account)? {
            authorities.push(account);
        }
    }
    Ok(authorities)
}

/// Check whether a TEE attestation quote hash has already been used in a
/// `MemberJoinedViaTeeAttestation` op for this group.
pub fn is_quote_hash_used(
    store: &Store,
    group_id: &ContextGroupId,
    quote_hash: &[u8; 32],
) -> EyreResult<bool> {
    let entries = read_op_log_after(store, group_id, 0, usize::MAX)?;

    for (seq, bytes) in &entries {
        // A swallowed decode failure here weakens REPLAY protection: an
        // unreadable entry that recorded this very quote reads as "not used".
        let Ok(op) = decode_group_op(group_id, *seq, bytes, "is_quote_hash_used") else {
            continue;
        };
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

    // ...and the NAMESPACE log, where a fleet replica's admission actually
    // lives. Without this the guard was inert for exactly the case it exists
    // to protect.
    //
    // `admit_tee_node` publishes two different ops. An already-bound namespace
    // member moving inward gets a `GroupOp` on the per-group log, which the
    // scan above sees. A FLEET REPLICA is an outsider joining the namespace,
    // so its admission is a SEALED `RootOp` on the namespace log -- and this
    // function never looked there, so `is_quote_hash_used` answered `false`
    // for every replica quote ever presented, including one it had just
    // admitted.
    //
    // `tee_admission_record` in this same module already scans both, which is
    // what the fan-in relies on; this one was left behind.
    //
    // Opening the sealed ops is best-effort, exactly as it is there: a node
    // without the namespace key reads nothing. That is the right shape here,
    // because the node that runs this check is the ADMITTER, which holds the
    // namespace key by definition.
    for root in root_ops_for(store, group_id)? {
        if let RootOp::MemberJoinedViaTeeAttestation {
            group_id: op_group,
            quote_hash: ref existing_hash,
            ..
        } = root
        {
            // The root form names its group explicitly, so a quote spent
            // admitting into a DIFFERENT group of this namespace must not read
            // as spent here.
            if op_group == *group_id && existing_hash == quote_hash {
                return Ok(true);
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

    for (seq, bytes) in &entries {
        let Ok(op) = decode_group_op(group_id, *seq, bytes, "is_tee_admitted_identity") else {
            continue;
        };
        if let GroupOp::MemberJoinedViaTeeAttestation { member, .. } = op.op {
            if member == *identity {
                return Ok(true);
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
fn tee_admission_from_bytes(
    group_id: &ContextGroupId,
    sequence: u64,
    bytes: &[u8],
    scan: &'static str,
) -> Option<(AccountId, TeeAdmissionRecord)> {
    if let Ok(op) = decode_group_op(group_id, sequence, bytes, scan) {
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

/// This namespace's root ops, for the namespace owning `group_id`, with the
/// sealed ones opened.
///
/// `MemberJoinedViaTeeAttestation` is published SEALED, so reading only the
/// cleartext ops finds no admission for any fleet replica — and the subgroup
/// fan-in below would then admit nobody, silently, on a namespace that has
/// plainly admitted them. Opening is best-effort: a node that does not hold the
/// namespace key cannot read the admission, which is the ordinary state before
/// its own key delivery rather than an error.
fn root_ops_for(store: &Store, group_id: &ContextGroupId) -> EyreResult<Vec<RootOp>> {
    let namespace = NamespaceRepository::new(store).resolve(group_id)?;
    let namespace_id = calimero_governance_types::NamespaceId::from(namespace.to_bytes());
    let mut ops = Vec::new();
    for signed in crate::NamespaceOpLogService::new(store, namespace_id).collect_root_ops()? {
        match signed.op {
            NamespaceOp::Root(root) => ops.push(root),
            NamespaceOp::RootSealed { key_id, encrypted } => {
                match crate::open_sealed_root_op(store, namespace_id, key_id.as_bytes(), &encrypted)
                {
                    Ok(Some(root)) => ops.push(root),
                    Ok(None) => {}
                    Err(e) => tracing::warn!(
                        namespace_id = %hex::encode(namespace.to_bytes()),
                        error = %format!("{e:#}"),
                        "skipping a sealed root op that would not open while reading TEE \
                         admissions"
                    ),
                }
            }
            // Skipped deliberately, not by omission. A subgroup-sealed op is
            // always a `MemberJoined` / `MemberJoinedAt`, never a
            // `MemberJoinedViaTeeAttestation` — that one is published by the
            // admitter under the namespace key — so opening it here would cost a
            // decrypt per op to find nothing this reader wants. An admitted TEE
            // node still reads its own subgroup's joins; it does so through the
            // ordinary apply path, not this admission scan.
            NamespaceOp::RootSealedForGroup { .. } => {}
            _ => {}
        }
    }
    Ok(ops)
}

/// The admission verdict a [`RootOp`] carries, if it is one and it names
/// `group_id`.
fn tee_admission_from_root_op(
    root: &RootOp,
    group_id: &ContextGroupId,
) -> Option<(AccountId, TeeAdmissionRecord)> {
    {
        if let RootOp::MemberJoinedViaTeeAttestation {
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
        } = root
        {
            // The root form names its group explicitly, so an admission into a
            // DIFFERENT group in this namespace must not be read as one into
            // `group_id`.
            if *op_group != *group_id {
                return None;
            }
            // The root form names the attested KEY, while the verdict is
            // recorded against the account that admission created. Both are on
            // the op: the account comes from the credential, which the apply
            // refused unless it certifies this very key.
            return Some((
                account.statement.account,
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

    for (seq, bytes) in &entries {
        if let Some((member, record)) =
            tee_admission_from_bytes(group_id, *seq, bytes, "tee_admission_record")
        {
            if member == *identity {
                latest = Some(record);
            }
        }
    }

    // ...and the namespace log, where the ROOT admission lives. A fleet
    // replica's admission is a `RootOp`, so it never appears in the per-group
    // log scanned above; without this the subgroup fan-in finds no verdict for
    // a replica the namespace has plainly admitted.
    for root in root_ops_for(store, group_id)? {
        if let Some((member, record)) = tee_admission_from_root_op(&root, group_id) {
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

    for (seq, bytes) in &entries {
        if let Some((member, record)) =
            tee_admission_from_bytes(group_id, *seq, bytes, "tee_admission_records")
        {
            // Last-write-wins: a later re-admission supersedes the earlier verdict.
            let _ = out.insert(member, record);
        }
    }

    for root in root_ops_for(store, group_id)? {
        if let Some((member, record)) = tee_admission_from_root_op(&root, group_id) {
            let _ = out.insert(member, record);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use calimero_account::AccountId;
    use calimero_context_client::local_governance::{GroupOp, NamespaceOp, SignedGroupOp};
    use calimero_context_config::types::ContextGroupId;
    use calimero_primitives::context::GroupMemberRole;
    use calimero_primitives::identity::PrivateKey;

    use calimero_primitives::identity::PublicKey;

    use super::{
        is_tee_authority, tee_admission_record, tee_admission_records, tee_evidence_owed,
        writer_account,
    };
    use crate::local_state::append_op_log_entry;
    use crate::test_fixtures::test_store;
    use crate::MembershipRepository;

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

    fn authoring_policy_op(
        signer_sk: &PrivateKey,
        ns_gid: ContextGroupId,
        nonce: u64,
        allowed_mrtd: &[&str],
    ) -> SignedGroupOp {
        SignedGroupOp::sign(
            signer_sk,
            ns_gid,
            vec![],
            nonce,
            GroupOp::TeeAuthoringPolicySet {
                allowed_mrtd: allowed_mrtd.iter().map(|m| (*m).to_owned()).collect(),
            },
        )
        .unwrap()
    }

    /// The MRTD a mock quote reports: 48 zero bytes.
    const MOCK_MRTD: &str =
        "000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

    /// A mock quote binding `key`, the way fleet join binds its own key.
    fn mock_quote_for(key: &PublicKey) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        let key_hash: [u8; 32] = Sha256::digest(**key).into();
        let report_data = calimero_tee_attestation::build_report_data(&[0x01; 32], Some(&key_hash));
        calimero_tee_attestation::generate_mock_attestation(report_data).quote_bytes
    }

    fn evidence_op(
        signer_sk: &PrivateKey,
        ns_gid: ContextGroupId,
        nonce: u64,
        member: AccountId,
        attested_key: PublicKey,
        quote: Vec<u8>,
    ) -> SignedGroupOp {
        SignedGroupOp::sign(
            signer_sk,
            ns_gid,
            vec![],
            nonce,
            GroupOp::TeeAuthorityEvidence {
                member,
                attested_key,
                quote,
                collateral: None,
                attested_at: 1_751_000_000,
            },
        )
        .unwrap()
    }

    /// One namespace with an admitted TEE (admission op, `ReadOnlyTee` row, key
    /// bound to its account) and a way to append ops to the root's log.
    struct Fixture {
        store: calimero_store::Store,
        ns_gid: ContextGroupId,
        signer_sk: PrivateKey,
        tee_key: PublicKey,
        tee: AccountId,
        seq: std::cell::Cell<u64>,
    }

    impl Fixture {
        fn new(ns_byte: u8) -> Self {
            let store = test_store();
            let ns_gid = ContextGroupId::from([ns_byte; 32]);
            let signer_sk = PrivateKey::random(&mut rand::rng());
            let (tee_key, tee) = crate::test_fixtures::enrolled(&store, &ns_gid, 0x70);
            let this = Self {
                store,
                ns_gid,
                signer_sk,
                tee_key,
                tee,
                seq: std::cell::Cell::new(0),
            };
            this.log(|sk, ns, n| tee_join_op(sk, ns, n, tee, [0x07; 32]));
            MembershipRepository::new(&this.store)
                .add_member(&ns_gid, &tee, GroupMemberRole::ReadOnlyTee)
                .unwrap();
            this
        }

        fn log(&self, op: impl FnOnce(&PrivateKey, ContextGroupId, u64) -> SignedGroupOp) {
            let seq = self.seq.get() + 1;
            self.seq.set(seq);
            let op = op(&self.signer_sk, self.ns_gid, seq);
            append_op_log_entry(&self.store, &self.ns_gid, seq, &borsh::to_vec(&op).unwrap())
                .unwrap();
        }

        fn policy(&self, allowed: &[&str]) {
            self.log(|sk, ns, n| authoring_policy_op(sk, ns, n, allowed));
        }

        fn evidence(&self, member: AccountId, attested_key: PublicKey, quote: Vec<u8>) {
            self.log(|sk, ns, n| evidence_op(sk, ns, n, member, attested_key, quote));
        }

        fn is_authority(&self, account: &AccountId) -> bool {
            is_tee_authority(&self.store, &self.ns_gid, account).unwrap()
        }
    }

    /// A TEE is an authority exactly while it is still a `ReadOnlyTee` member,
    /// holds verified evidence, and the latest authoring policy names the MRTD
    /// that evidence's quote reports.
    #[test]
    fn tee_authority_follows_the_latest_policy_and_membership() {
        let f = Fixture::new(0xAC);
        f.evidence(f.tee, f.tee_key, mock_quote_for(&f.tee_key));

        assert!(!f.is_authority(&f.tee), "no policy: TEE authorship is off");
        f.policy(&["m2"]);
        assert!(
            !f.is_authority(&f.tee),
            "a policy that does not name the quote's MRTD admits no authority"
        );
        f.policy(&["m2", MOCK_MRTD]);
        assert!(f.is_authority(&f.tee));
        f.policy(&[]);
        assert!(
            !f.is_authority(&f.tee),
            "an empty list turns TEE authorship back off"
        );
        f.policy(&[MOCK_MRTD]);
        assert!(f.is_authority(&f.tee));
        MembershipRepository::new(&f.store)
            .remove_member(&f.ns_gid, &f.tee)
            .unwrap();
        assert!(
            !f.is_authority(&f.tee),
            "the admission record outlives a removal; the authority must not"
        );
    }

    /// Evidence is owed to an admitted TEE only while authorship is on and none
    /// is recorded, whatever MRTD the policy names, and never to anyone else.
    #[test]
    fn evidence_is_owed_only_to_an_admitted_tee_without_it_while_authorship_is_on() {
        let f = Fixture::new(0xAD);
        let owed = |account: &AccountId| tee_evidence_owed(&f.store, &f.ns_gid, account).unwrap();
        let (_key, member) = crate::test_fixtures::enrolled(&f.store, &f.ns_gid, 0x73);
        MembershipRepository::new(&f.store)
            .add_member(&f.ns_gid, &member, GroupMemberRole::Member)
            .unwrap();

        assert!(!owed(&f.tee), "authorship off: nothing is owed");
        f.policy(&["another-image"]);
        assert!(
            owed(&f.tee),
            "owed even when the policy does not name this TEE's image"
        );
        assert!(!owed(&member), "a member is never owed TEE evidence");

        f.evidence(f.tee, f.tee_key, mock_quote_for(&f.tee_key));
        assert!(!owed(&f.tee), "recorded evidence settles it");

        let g = Fixture::new(0xAE);
        g.policy(&[MOCK_MRTD]);
        assert!(tee_evidence_owed(&g.store, &g.ns_gid, &g.tee).unwrap());
        MembershipRepository::new(&g.store)
            .remove_member(&g.ns_gid, &g.tee)
            .unwrap();
        assert!(
            !tee_evidence_owed(&g.store, &g.ns_gid, &g.tee).unwrap(),
            "a removed TEE is owed nothing"
        );
    }

    /// The attack evidence exists to stop. Any member may sign an admission op,
    /// and peers check only its claimed measurements. A member who forges one
    /// for an account it controls, claiming exactly the MRTD the policy names,
    /// gets a `ReadOnlyTee` row but no authority: it has no quote to prove it.
    #[test]
    fn a_forged_admission_without_evidence_is_not_an_authority() {
        let f = Fixture::new(0xAF);
        let (_key, forged) = crate::test_fixtures::enrolled(&f.store, &f.ns_gid, 0x71);
        f.log(|sk, ns, n| {
            SignedGroupOp::sign(
                sk,
                ns,
                vec![],
                n,
                GroupOp::MemberJoinedViaTeeAttestation {
                    member: forged,
                    quote_hash: [0x66; 32],
                    mrtd: MOCK_MRTD.to_owned(),
                    rtmr0: "r0".to_owned(),
                    rtmr1: "r1".to_owned(),
                    rtmr2: "r2".to_owned(),
                    rtmr3: "r3".to_owned(),
                    tcb_status: "UpToDate".to_owned(),
                    role: GroupMemberRole::ReadOnlyTee,
                },
            )
            .unwrap()
        });
        MembershipRepository::new(&f.store)
            .add_member(&f.ns_gid, &forged, GroupMemberRole::ReadOnlyTee)
            .unwrap();
        f.policy(&[MOCK_MRTD]);

        assert!(!f.is_authority(&forged));
    }

    /// Evidence names the key its quote binds. Copying a genuine TEE's evidence
    /// and relabelling it for another account does not work, because that key
    /// does not speak for the other account.
    #[test]
    fn evidence_relabelled_for_another_account_is_ignored() {
        let f = Fixture::new(0xB0);
        let (_key, other) = crate::test_fixtures::enrolled(&f.store, &f.ns_gid, 0x72);
        MembershipRepository::new(&f.store)
            .add_member(&f.ns_gid, &other, GroupMemberRole::ReadOnlyTee)
            .unwrap();
        f.log(|sk, ns, n| tee_join_op(sk, ns, n, other, [0x08; 32]));
        f.policy(&[MOCK_MRTD]);
        f.evidence(other, f.tee_key, mock_quote_for(&f.tee_key));

        assert!(!f.is_authority(&other));
    }

    /// A quote proves one key. Evidence that names a different key than the one
    /// the quote binds does not verify.
    #[test]
    fn evidence_whose_quote_binds_another_key_is_ignored() {
        let f = Fixture::new(0xB1);
        let stranger = PublicKey::from([0x99; 32]);
        f.policy(&[MOCK_MRTD]);
        f.evidence(f.tee, f.tee_key, mock_quote_for(&stranger));

        assert!(!f.is_authority(&f.tee));
    }

    /// An ordinary member is never an authority, even with an admission record,
    /// genuine-looking evidence and a matching policy: only the
    /// attestation-minted `ReadOnlyTee` role is.
    #[test]
    fn a_member_with_a_matching_record_is_not_a_tee_authority() {
        let f = Fixture::new(0xAD);
        let (key, member) = crate::test_fixtures::enrolled(&f.store, &f.ns_gid, 0x73);
        f.log(|sk, ns, n| tee_join_op(sk, ns, n, member, [0x09; 32]));
        MembershipRepository::new(&f.store)
            .add_member(&f.ns_gid, &member, GroupMemberRole::Member)
            .unwrap();
        f.evidence(member, key, mock_quote_for(&key));
        f.policy(&[MOCK_MRTD]);

        assert!(!f.is_authority(&member));
    }

    /// Only the key the quote binds acts as the TEE authority. Members, admins,
    /// a removed TEE, and any other key of the TEE's own account resolve to
    /// their own account, so none of them can match a `TeeOnly` writer set.
    #[test]
    fn only_the_attested_key_resolves_to_the_tee_authority() {
        let f = Fixture::new(0xAE);
        let (member_key, member) = crate::test_fixtures::enrolled(&f.store, &f.ns_gid, 0x74);
        let (admin_key, admin) = crate::test_fixtures::enrolled(&f.store, &f.ns_gid, 0x75);
        let membership = MembershipRepository::new(&f.store);
        membership
            .add_member(&f.ns_gid, &member, GroupMemberRole::Member)
            .unwrap();
        membership
            .add_member(&f.ns_gid, &admin, GroupMemberRole::Admin)
            .unwrap();
        f.evidence(f.tee, f.tee_key, mock_quote_for(&f.tee_key));
        let resolve =
            |key: &PublicKey, account| writer_account(&f.store, &f.ns_gid, key, account).unwrap();

        // Authorship off: nobody is the authority, the TEE included.
        assert_eq!(resolve(&f.tee_key, f.tee), f.tee);

        f.policy(&[MOCK_MRTD]);
        assert_eq!(resolve(&f.tee_key, f.tee), AccountId::TEE_AUTHORITY);
        assert_eq!(
            resolve(&member_key, member),
            member,
            "a member is never the authority"
        );
        assert_eq!(resolve(&admin_key, admin), admin, "nor is an admin");
        let other_key = PublicKey::from([0x98; 32]);
        assert_eq!(
            resolve(&other_key, f.tee),
            f.tee,
            "a key the quote does not bind is not the authority, even for the TEE's account"
        );

        membership.remove_member(&f.ns_gid, &f.tee).unwrap();
        assert_eq!(
            resolve(&f.tee_key, f.tee),
            f.tee,
            "a removed TEE loses the authority"
        );
    }

    #[test]
    fn record_returns_stored_verdict_for_admitted_member() {
        let store = test_store();
        let mut rng = rand::rng();
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

    /// A fleet replica's admission is a SEALED root op, and the fan-in has to
    /// open it.
    ///
    /// `MemberJoinedViaTeeAttestation` is published sealed under the namespace
    /// key, so a read that only looks at cleartext root ops finds no admission
    /// for any replica. Nothing errors: `tee_admission_record` answers `None`,
    /// the subgroup fan-in reads that as "not admitted", and every Restricted
    /// subgroup created afterwards silently admits nobody -- on a namespace that
    /// has plainly admitted them.
    #[test]
    fn a_sealed_root_admission_is_opened_and_read_back() {
        use calimero_context_client::local_governance::{RootOp, SignedNamespaceOp};

        use crate::test_fixtures::{real_join_account, seal_for_test};

        let store = test_store();
        let mut rng = rand::rng();
        let namespace_id = [0xAB; 32];
        let ns_gid = ContextGroupId::from(namespace_id);

        let replica_sk = PrivateKey::random(&mut rng);
        let replica = replica_sk.public_key();
        let account = real_join_account(&replica);
        let replica_account = account.statement.account;

        // Signed by the ADMITTER, which is who publishes this op.
        let admitter_sk = PrivateKey::random(&mut rng);
        let admit = SignedNamespaceOp::sign(
            &admitter_sk,
            namespace_id.into(),
            vec![],
            1,
            seal_for_test(
                &store,
                ns_gid,
                RootOp::MemberJoinedViaTeeAttestation {
                    group_id: ns_gid,
                    member: replica,
                    quote_hash: [0x0A; 32],
                    mrtd: "m1".to_owned(),
                    rtmr0: "r0".to_owned(),
                    rtmr1: "r1".to_owned(),
                    rtmr2: "r2".to_owned(),
                    rtmr3: "r3".to_owned(),
                    tcb_status: "UpToDate".to_owned(),
                    role: GroupMemberRole::ReadOnlyTee,
                    account,
                },
            ),
        )
        .expect("the admitter signs the admission");
        assert!(
            matches!(admit.op, NamespaceOp::RootSealed { .. }),
            "precondition: this variant must be published sealed, or the test \
             proves nothing"
        );
        crate::NamespaceOpLogService::new(&store, namespace_id.into())
            .store_signed_operation(&admit)
            .expect("land the admission on the namespace log");

        let record = tee_admission_record(&store, &ns_gid, &replica_account)
            .unwrap()
            .expect("a sealed admission this node holds the key for must read back");
        assert_eq!(record.quote_hash, [0x0A; 32]);
        assert_eq!(record.role, GroupMemberRole::ReadOnlyTee);
        assert_eq!(
            tee_admission_records(&store, &ns_gid)
                .unwrap()
                .get(&replica_account)
                .map(|r| r.quote_hash),
            Some([0x0A; 32]),
            "and the batch read must agree with the single-member one"
        );
    }

    #[test]
    fn records_returns_all_verdicts_in_one_scan() {
        let store = test_store();
        let mut rng = rand::rng();
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
        let mut rng = rand::rng();
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
