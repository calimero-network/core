//! The account anchor, and a member's endorsement of one.
//!
//! [`AccountGenesis`] is the immutable value an [`AccountId`] addresses — the
//! anchor every other credential in this crate is checked against.
//! [`AccountMemberEndorsement`] is the separate statement a *granted member key*
//! makes about an account, which is how an account whose root is a member
//! nowhere can still be linked into a scope.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey, PublicKey};

use crate::domain::{borsh_bytes, ACCOUNT_ENDORSEMENT_SIGN_DOMAIN, ACCOUNT_ID_DOMAIN};
use crate::error::AccountError;

/// Version tag written into [`AccountGenesis`]. It is part of the preimage of
/// [`AccountId`], so bumping it makes every id under the new version distinct
/// from every id under the old one — a deliberate hard fork of the namespace
/// rather than a silent reinterpretation of existing ids.
///
/// `2` since the genesis dropped its per-namespace nonce. The field's removal
/// already changes the preimage, but a version that moved with it is what makes
/// a v1 id and a v2 id from the same root provably different values rather than
/// two encodings a reader might reconcile.
pub const ACCOUNT_GENESIS_VERSION: u8 = 2;

/// The immutable root of an account. Hashing it yields the [`AccountId`], which
/// is why a verifier can recover the epoch-0 root key from the id alone.
///
/// One root key means one account, everywhere. There is no per-scope salt: an
/// account that differed per namespace would only be worth its cost if the
/// difference were unlinkable, and it is not — the genesis names the same
/// `root_sign_pk` in every namespace it is published to, so a member of two of
/// them links the accounts by comparing two numbers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountGenesis {
    /// Always [`ACCOUNT_GENESIS_VERSION`] for accounts this build mints.
    pub version: u8,
    /// The account's epoch-0 root signing key.
    pub root_sign_pk: PublicKey,
}

impl AccountGenesis {
    /// Build a genesis at the current [`ACCOUNT_GENESIS_VERSION`].
    #[must_use]
    pub const fn new(root_sign_pk: PublicKey) -> Self {
        Self {
            version: ACCOUNT_GENESIS_VERSION,
            root_sign_pk,
        }
    }

    /// The [`AccountId`] this genesis addresses.
    #[must_use]
    pub fn account_id(&self) -> AccountId {
        AccountId::from(domain_hash(ACCOUNT_ID_DOMAIN, &[&borsh_bytes(self)]))
    }
}

/// A group member's statement that an account is theirs.
///
/// Exists because an account root is deliberately **not** a member key. The root is
/// kept offline so it survives losing every device, which means the link gate
/// cannot ask "is this account's root a member?" — it is a member nowhere. Instead
/// the member key that *is* granted signs the account id, and the gate asks whether
/// the **endorser** is a member and whether it really signed this account.
///
/// Equally strong as the old question: only a member can produce a valid
/// endorsement, and only the root holder can certify devices into the account. It
/// takes both to enroll, and neither alone is enough.
///
/// Anyone may endorse an account they do not own — the account id is public — and it
/// gains them nothing, exactly as constructing a genesis over someone else's key
/// gains nothing: enrolling a device still needs the root's signature.
#[derive(Clone, Copy, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountMemberEndorsement {
    /// The account being endorsed.
    pub account: AccountId,
    /// The member key making the statement. Must be a member at the op's cut.
    pub member: PublicKey,
    /// `member`'s signature over [`Self::signing_payload`].
    pub signature: [u8; 64],
}

impl AccountMemberEndorsement {
    /// Canonical bytes an endorser signs.
    ///
    /// Covers the account id and the endorsing key. Including the endorser is what
    /// stops a valid endorsement being re-presented as though a *different* member
    /// had made it — the signature would then verify against a key that never
    /// signed anything.
    #[must_use]
    pub fn signing_payload(account: AccountId, member: &PublicKey) -> [u8; 32] {
        domain_hash(
            ACCOUNT_ENDORSEMENT_SIGN_DOMAIN,
            &[account.as_bytes(), AsRef::<[u8; 32]>::as_ref(member)],
        )
    }
}

/// Sign an endorsement of `account` with a granted member key.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key cannot sign.
pub fn sign_account_endorsement(
    member_sk: &PrivateKey,
    account: AccountId,
) -> Result<AccountMemberEndorsement, AccountError> {
    let member = member_sk.public_key();
    let payload = AccountMemberEndorsement::signing_payload(account, &member);
    Ok(AccountMemberEndorsement {
        account,
        member,
        signature: member_sk
            .sign(&payload)
            .map_err(|_| AccountError::SigningFailed)?
            .to_bytes(),
    })
}

/// Check an endorsement is internally valid — that `member` really signed
/// `account`.
///
/// Says nothing about whether `member` is actually a member: that is an at-cut
/// question only the projection can answer, and keeping the two apart is the same
/// split as [`crate::VerifiedDeviceCert`] versus "in force".
///
/// # Errors
/// [`AccountError::EndorsementSignatureInvalid`] if the signature does not verify.
pub fn verify_account_endorsement(
    endorsement: &AccountMemberEndorsement,
) -> Result<(), AccountError> {
    let payload =
        AccountMemberEndorsement::signing_payload(endorsement.account, &endorsement.member);
    endorsement
        .member
        .verify_raw_signature(&payload, &endorsement.signature)
        .map_err(|_| AccountError::EndorsementSignatureInvalid)
}
