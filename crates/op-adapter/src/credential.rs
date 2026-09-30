//! The join-credential predicates — the op-local half of credential admission,
//! shared verbatim by this crate's encoders and the governance apply path.
//!
//! Both planes have to reach the same verdict about a credential, or a device is
//! bound in the folded view and absent from the materialized rows. One predicate
//! per join shape, called from both places, is how that is guaranteed rather
//! than hoped for.

use calimero_account::AccountId;
use calimero_governance_types::{JoinAccountCredential, RootOp};
use calimero_primitives::identity::PublicKey;

/// Whether a join op's credential actually belongs to the member it admits —
/// the one predicate both the projection encoder and the governance apply path
/// use.
///
/// Two questions, and both have to be asked in both places or the planes split:
///
/// 1. **Does it name this member?** `cert.account == member`. Now that a join op
///    names an ACCOUNT rather than a key, this is the ownership check outright:
///    a credential lifted from another join names a different account and simply
///    fails to match. When the field was a key, the same question needed
///    `cert.sign_pk == member` instead.
/// 2. **Is it internally valid?** `AccountProof::verify` checks that the genesis
///    hashes to the account the certificate claims and that the certificate is
///    signed by the root key its chain reaches — the same check `apply_link` makes.
///
/// The remaining half — that the op was SIGNED by the device this certificate
/// certifies — needs the signer, which only the apply path has, and which the
/// TEE admission deliberately answers differently (a replica cannot admit
/// itself, so its op is signed by the verifier). It therefore lives there.
///
/// Deliberately **op-local only**. Revocation and epoch supersession are stateful,
/// so they cannot be answered from the op alone; the apply path answers them from
/// its rows and the projection from its fold, and both refuse the same
/// credentials. Keeping this function to the decidable half is what lets one
/// predicate serve both without either pretending to know the other's state.
#[must_use]
pub fn join_credential_binds(member: &AccountId, credential: &JoinAccountCredential) -> bool {
    credential.statement.account == *member && credential.verify(*member).is_ok()
}

/// The same predicate for the one join op that names a **key**.
///
/// `RootOp::MemberJoinedViaTeeAttestation` names the attested replica's signing
/// key rather than its account, and it has to: the quote's `report_data` binds
/// to that key, which is what stops a captured quote being replayed for a
/// different identity. The account it joins as therefore comes from the
/// credential, and the ownership question becomes "does this credential certify
/// the key the quote attested" — the mirror of
/// [`join_credential_binds`]'s `cert.account == member`.
///
/// Without it, a verifier could pair a genuine quote for one replica with a
/// credential minted for an entirely different account, and admit the wrong
/// principal on the strength of somebody else's attestation.
#[must_use]
pub fn join_credential_certifies(member: &PublicKey, credential: &JoinAccountCredential) -> bool {
    credential.statement.sign_pk == *member
        && credential.verify(credential.statement.account).is_ok()
}

/// The value an admission quote's report data must carry in bytes `32..64` for
/// `credential` to be admitted as `member` into `group_id` of `namespace_id`.
///
/// One definition for the joiner that generates the quote, the verifier that
/// checks it and every peer that applies the admission, so none of them can
/// disagree about what the quote commits to.
#[must_use]
pub fn tee_admission_binding(
    namespace_id: &[u8; 32],
    group_id: &[u8; 32],
    member: &PublicKey,
    credential: &JoinAccountCredential,
) -> [u8; 32] {
    let statement = &credential.statement;
    calimero_tee_attestation::admission_binding(
        namespace_id,
        group_id,
        member,
        statement.account.as_bytes(),
        statement.kem_pk.as_bytes(),
        statement.device.as_bytes(),
    )
}

/// Whether `quote` commits to exactly `credential` admitted as `member` into
/// `group_id` of `namespace_id`. A structural read of the quote's report data:
/// the signature is the verifier's check, this is what every peer can repeat.
#[must_use]
pub fn tee_quote_binds_credential(
    namespace_id: &[u8; 32],
    group_id: &[u8; 32],
    member: &PublicKey,
    credential: &JoinAccountCredential,
    quote: &[u8],
) -> bool {
    calimero_tee_attestation::quote_report_data(quote).is_ok_and(|report_data| {
        report_data[32..] == tee_admission_binding(namespace_id, group_id, member, credential)
    })
}

/// [`join_credential_binds`] applied to whichever join variant `op` is.
pub(crate) fn credential_binds_the_member(op: &RootOp) -> bool {
    match op {
        RootOp::MemberJoined {
            member, account, ..
        }
        | RootOp::MemberJoinedAt {
            member, account, ..
        }
        | RootOp::MemberJoinedOpen {
            member, account, ..
        } => join_credential_binds(member, account),
        // The TEE admission is the one join that names a KEY rather than an
        // account: the quote's `report_data` binds to the attested key, so the
        // op has to name it, and the account comes from the credential beside
        // it. The ownership question is therefore the mirrored one — does this
        // credential certify the key the quote attested.
        RootOp::MemberJoinedViaTeeAttestation {
            member, account, ..
        } => join_credential_certifies(member, account),
        // Genesis binds the FOUNDER's device, and the founder names an account
        // like any other member — so the same "does this credential speak for
        // the principal the op names" question applies unchanged.
        RootOp::NamespaceCreatedV2 {
            founder, account, ..
        } => join_credential_binds(founder, account),
        _ => false,
    }
}
