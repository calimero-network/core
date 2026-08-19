//! Linking a new device: the key material it minted, the statement it signs over
//! that material, and the short code two humans compare out of band.
//!
//! # Why it is shaped this way
//!
//! **The four values travel together, so they are one type.** An account, a
//! device, and the two keys being offered are what a pairing *is*; a caller
//! holding three of the four can neither sign nor check anything. Naming the
//! quadruple as [`PairingOffer`] is what stops both ends re-listing it at every
//! call, and re-listing it is how the two ends come to disagree about which four
//! values a signature covers.
//!
//! **The statement and the code cover different attacks, and neither replaces the
//! other.** Without the statement, `pair-complete` certifies whatever keys arrive
//! beside a [`DeviceId`]. An attacker cannot mint a `DeviceId` — it is
//! `H(account ‖ nonce)` and the nonce never leaves the pairing node — but it can
//! substitute key material *under* a captured one, and the resulting certificate
//! names the attacker's keys as a trusted device of somebody else's account.
//!
//! The statement refuses the *partial* substitution: swapping the KEM key while
//! keeping the real signing key breaks the signature, and the attacker cannot
//! re-sign without that key. It does **not** refuse a wholesale one — an attacker
//! that replaces both keys and re-signs with its own produces a statement that
//! verifies, because nothing in it commits to the genuine keys in advance. Binding
//! the keys into the `DeviceId` would fix that, and is deliberately unavailable:
//! [`DeviceId::mint`] excludes them so a device keeps its replica identity across
//! key rotation.
//!
//! [`PairingOffer::confirmation_code`] covers the remaining case, by giving the two
//! humans a value to compare that an attacker cannot reproduce.
//!
//! **The code's length is its work factor.** The attacker sees the genuine payload,
//! so it knows the target code and can grind its own keypairs offline until one
//! matches. 64 bits puts that at roughly 2^64 hashes, whereas the six digits a
//! human would prefer to read is 2^20 — instant. Grouped in fours so it can be
//! compared by eye and read aloud without losing the length that makes it worth
//! comparing.

use calimero_primitives::identity::{domain_hash, AccountId, DeviceId, PrivateKey, PublicKey};

use crate::device::KemPublicKey;
use crate::domain::{
    PAIRING_CONFIRMATION_DOMAIN, PAIRING_CONFIRMATION_HEX_LEN, PAIRING_STATEMENT_SIGN_DOMAIN,
};
use crate::error::AccountError;
use crate::signed::sign_payload;

/// The key material a pairing device minted, and the identity it minted it for.
///
/// Both ends of a pairing build one of these — the pairing device from what it
/// generated, the certifying side from what arrived — and every question either
/// end asks is a method on it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PairingOffer {
    /// The account the device is joining.
    pub account: AccountId,
    /// The device's replica id.
    pub device: DeviceId,
    /// X25519 key wrapped scope keys will be delivered to.
    pub kem_pk: KemPublicKey,
    /// Ed25519 key the device will sign ops with.
    pub sign_pk: PublicKey,
}

impl PairingOffer {
    /// An offer over key material the caller received.
    ///
    /// This is the *verifying* side's constructor: it names `sign_pk` because it
    /// does not hold the matching secret. The pairing side should use
    /// [`Self::signed`], which proves possession instead of asserting it.
    #[must_use]
    pub const fn new(
        account: AccountId,
        device: DeviceId,
        kem_pk: KemPublicKey,
        sign_pk: PublicKey,
    ) -> Self {
        Self {
            account,
            device,
            kem_pk,
            sign_pk,
        }
    }

    /// Mint an offer for `device_sk`'s public key, with the statement proving the
    /// minter holds it.
    ///
    /// `sign_pk` is derived from `device_sk` rather than taken as an argument: the
    /// statement is a proof of possession, so a caller that could name a key it
    /// does not hold would defeat the point. Getting a statement at all therefore
    /// requires handing over the secret.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key cannot sign.
    pub fn signed(
        device_sk: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        kem_pk: KemPublicKey,
    ) -> Result<(Self, [u8; 64]), AccountError> {
        let offer = Self::new(account, device, kem_pk, device_sk.public_key());
        let statement = sign_payload(device_sk, &offer.payload())?;
        Ok((offer, statement))
    }

    /// Canonical bytes the pairing device signs.
    ///
    /// Covers the account it is joining, its own replica id, and **both** keys the
    /// certificate will name. The account is in the preimage so a statement
    /// produced for one account cannot be presented while pairing into another;
    /// the keys are there because they are the entire content of what gets
    /// certified.
    #[must_use]
    pub fn payload(&self) -> [u8; 32] {
        domain_hash(
            PAIRING_STATEMENT_SIGN_DOMAIN,
            &[
                self.account.as_bytes(),
                self.device.as_bytes(),
                self.kem_pk.as_bytes(),
                AsRef::<[u8; 32]>::as_ref(&self.sign_pk),
            ],
        )
    }

    /// Check that the party offering this key material is the party that generated
    /// it — that `signature` is [`Self::sign_pk`]'s over exactly these four values.
    ///
    /// See the module docs for what this closes and what it does not.
    ///
    /// # Errors
    /// [`AccountError::PairingStatementInvalid`] if the signature does not verify.
    pub fn verify_statement(&self, signature: &[u8; 64]) -> Result<(), AccountError> {
        self.sign_pk
            .verify_raw_signature(&self.payload(), signature)
            .map_err(|_| AccountError::PairingStatementInvalid)
    }

    /// A short value both ends derive independently, for the two humans to compare
    /// out of band.
    ///
    /// Equal codes mean the same key material is on both ends, which is the one
    /// thing no signature can establish — a substituting attacker can always
    /// re-sign, but it cannot make its own keys hash to the code the other side is
    /// reading.
    #[must_use]
    pub fn confirmation_code(&self) -> String {
        let digest = domain_hash(
            PAIRING_CONFIRMATION_DOMAIN,
            &[
                self.account.as_bytes(),
                self.device.as_bytes(),
                self.kem_pk.as_bytes(),
                AsRef::<[u8; 32]>::as_ref(&self.sign_pk),
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

    /// Whether `supplied` is the confirmation code for this offer.
    ///
    /// Comparison is on the hex digits only, upper-cased, so the grouping dashes
    /// and whatever case a person typed do not decide a security question.
    ///
    /// This is the check that makes the code more than advice: the account holder
    /// supplies the code they were *read* — from the pairing device's own output —
    /// and the certifying side derives one from the offer that actually arrived. A
    /// substituting attacker's keys derive a different code, so the two disagree
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
    pub fn code_matches(&self, supplied: &str) -> bool {
        let normalize = |code: &str| -> String {
            code.chars()
                .filter(char::is_ascii_hexdigit)
                .map(|c| c.to_ascii_uppercase())
                .collect()
        };
        let supplied = normalize(supplied);
        // A caller that stripped the code to nothing must not match a code that
        // normalizes to nothing either — refuse empty outright.
        !supplied.is_empty() && supplied == normalize(&self.confirmation_code())
    }
}
