//! Linking a new device: the statement it signs over the key material it
//! minted, and the short code two humans compare out of band.
//!
//! The two cover different attacks and neither replaces the other — the
//! statement refuses a *partial* substitution, the code is what still refuses a
//! wholesale one.

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey, PublicKey};

use crate::device::KemPublicKey;
use crate::domain::{PAIRING_CONFIRMATION_DOMAIN, PAIRING_STATEMENT_SIGN_DOMAIN};
use crate::error::AccountError;

/// Number of hex characters in a [`pairing_confirmation_code`], excluding its
/// separators. Eight bytes of digest.
pub(crate) const PAIRING_CONFIRMATION_HEX_LEN: usize = 16;

/// Canonical bytes a pairing device signs over the key material it minted.
///
/// Covers the account it is joining, its own replica id, and **both** keys the
/// certificate will name. The account is in the preimage so a statement produced
/// for one account cannot be presented while pairing into another; the keys are
/// there because they are the entire content of what gets certified.
#[must_use]
pub fn pairing_statement_payload(
    account: AccountId,
    device: DeviceId,
    kem_pk: &KemPublicKey,
    sign_pk: &PublicKey,
) -> [u8; 32] {
    domain_hash(
        PAIRING_STATEMENT_SIGN_DOMAIN,
        &[
            account.as_bytes(),
            device.as_bytes(),
            kem_pk.as_bytes(),
            AsRef::<[u8; 32]>::as_ref(sign_pk),
        ],
    )
}

/// Sign the pairing statement with the device's own signing key.
///
/// `sign_pk` is derived from `device_sk` rather than passed in: the statement is
/// a proof of possession, so a caller that could name a key it does not hold
/// would defeat the point.
///
/// # Errors
/// [`AccountError::SigningFailed`] if the key cannot sign.
pub fn sign_pairing_statement(
    device_sk: &PrivateKey,
    account: AccountId,
    device: DeviceId,
    kem_pk: &KemPublicKey,
) -> Result<[u8; 64], AccountError> {
    let sign_pk = device_sk.public_key();
    let payload = pairing_statement_payload(account, device, kem_pk, &sign_pk);
    Ok(device_sk
        .sign(&payload)
        .map_err(|_| AccountError::SigningFailed)?
        .to_bytes())
}

/// Check that the party offering this key material is the party that generated
/// it — that `signature` is `sign_pk`'s over exactly these four values.
///
/// # What this closes, and what it does not
///
/// Without it, `pair-complete` certifies whatever keys arrive beside a
/// `DeviceId`. An attacker cannot mint a `DeviceId` (it is `H(account ‖ nonce)`
/// and the nonce never leaves the pairing node), but it can substitute key
/// material *under* a captured one, and the resulting certificate names the
/// attacker's keys as a trusted device of somebody else's account.
///
/// This refuses the partial substitution: swapping the KEM key while keeping the
/// real signing key breaks the signature, and the attacker cannot re-sign
/// without that key. It does **not** by itself refuse a wholesale substitution —
/// an attacker that replaces both keys and re-signs with its own produces a
/// statement that verifies, because nothing here commits to the genuine keys in
/// advance. Binding the keys into the `DeviceId` would fix that, and is
/// deliberately not available: [`DeviceId::mint`] excludes them so a device
/// keeps its replica identity across key rotation.
///
/// [`pairing_confirmation_code`] is what covers the remaining case, by giving
/// the two humans a value to compare that an attacker cannot reproduce.
///
/// # Errors
/// [`AccountError::PairingStatementInvalid`] if the signature does not verify.
pub fn verify_pairing_statement(
    account: AccountId,
    device: DeviceId,
    kem_pk: &KemPublicKey,
    sign_pk: &PublicKey,
    signature: &[u8; 64],
) -> Result<(), AccountError> {
    let payload = pairing_statement_payload(account, device, kem_pk, sign_pk);
    sign_pk
        .verify_raw_signature(&payload, signature)
        .map_err(|_| AccountError::PairingStatementInvalid)
}

/// A short value both ends of a pairing derive independently, for the two humans
/// to compare out of band.
///
/// Both sides compute it from what they hold: the pairing device from what it
/// minted, the account holder from what arrived. Equal codes mean the same key
/// material is on both ends, which is the one thing no signature can establish —
/// a substituting attacker can always re-sign, but it cannot make its own keys
/// hash to the code the other side is reading.
///
/// # Why it is this long
///
/// The attacker sees the genuine payload, so it knows the target code and can
/// grind its own keypairs offline until one matches. The code's length *is* the
/// work factor: 64 bits puts that at roughly 2^64 hashes, whereas the six digits
/// a human would prefer to read is 2^20 — instant. Grouped in fours so it can be
/// compared by eye and read aloud without losing the length that makes it worth
/// comparing.
#[must_use]
pub fn pairing_confirmation_code(
    account: AccountId,
    device: DeviceId,
    kem_pk: &KemPublicKey,
    sign_pk: &PublicKey,
) -> String {
    let digest = domain_hash(
        PAIRING_CONFIRMATION_DOMAIN,
        &[
            account.as_bytes(),
            device.as_bytes(),
            kem_pk.as_bytes(),
            AsRef::<[u8; 32]>::as_ref(sign_pk),
        ],
    );
    let hex: String = digest
        .iter()
        .take(PAIRING_CONFIRMATION_HEX_LEN / 2)
        .map(|byte| format!("{byte:02X}"))
        .collect();
    hex.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join("-")
}

/// Whether `supplied` is the confirmation code for this key material.
///
/// Comparison is on the hex digits only, upper-cased, so the grouping dashes and
/// whatever case a person typed do not decide a security question.
///
/// This is the check that makes the code more than advice: the account holder
/// supplies the code they were *read* — from the pairing device's own output —
/// and the certifying side derives one from the payload that actually arrived.
/// A substituting attacker's keys derive a different code, so the two disagree
/// and the pairing is refused.
///
/// Its strength is exactly the independence of the two channels. A code that
/// travelled beside the keys it describes proves nothing — an attacker
/// rewriting the payload rewrites the code with it. What requiring it does buy,
/// unconditionally, is that the comparison can no longer be skipped by an
/// operator in a hurry.
///
/// No constant-time comparison: the code is derived from public values and the
/// attacker already knows the genuine one. There is no secret here to leak.
#[must_use]
pub fn pairing_code_matches(
    supplied: &str,
    account: AccountId,
    device: DeviceId,
    kem_pk: &KemPublicKey,
    sign_pk: &PublicKey,
) -> bool {
    let normalize = |code: &str| -> String {
        code.chars()
            .filter(|c| c.is_ascii_hexdigit())
            .map(|c| c.to_ascii_uppercase())
            .collect()
    };
    let expected = pairing_confirmation_code(account, device, kem_pk, sign_pk);
    let supplied = normalize(supplied);
    // A caller that stripped the code to nothing must not match a code that
    // normalizes to nothing either — refuse empty outright.
    !supplied.is_empty() && supplied == normalize(&expected)
}
