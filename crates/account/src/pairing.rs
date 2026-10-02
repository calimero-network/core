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
//! **The statement is dated, so a captured one goes stale.** The issue time is
//! inside the signed bytes, and the certifying side refuses a statement older
//! than [`PAIRING_STATEMENT_MAX_AGE_SECS`]. Without it, an offer captured today
//! could be completed whenever the account holder next runs `pair-complete`.
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

/// How long after a device signs its statement the certifying side still accepts
/// it. Long enough for two people to read a code to each other.
pub const PAIRING_STATEMENT_MAX_AGE_SECS: u64 = 300;

/// How far ahead of the verifier's clock a statement's issue time may sit, to
/// absorb the two nodes' clocks disagreeing.
pub const PAIRING_STATEMENT_MAX_SKEW_SECS: u64 = 60;

/// A pairing device's signature over its [`PairingOffer`] and the time it signed.
///
/// One opaque value on the wire: the issue time and the signature travel
/// together, so relaying it needs no field beyond the statement itself.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PairingStatement {
    issued_at: u64,
    signature: [u8; 64],
}

impl PairingStatement {
    /// Length of [`Self::to_bytes`]: eight bytes of issue time, then the signature.
    pub const LEN: usize = 72;

    /// Unix seconds at which the pairing device signed this statement.
    #[must_use]
    pub const fn issued_at(&self) -> u64 {
        self.issued_at
    }

    /// The wire form: the issue time big-endian, then the signature.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; Self::LEN] {
        let mut bytes = [0u8; Self::LEN];
        bytes[..8].copy_from_slice(&self.issued_at.to_be_bytes());
        bytes[8..].copy_from_slice(&self.signature);
        bytes
    }

    /// Read the wire form back. Whether it verifies is [`PairingOffer::verify_statement`]'s call.
    #[must_use]
    pub fn from_bytes(bytes: &[u8; Self::LEN]) -> Self {
        let mut issued_at = [0u8; 8];
        issued_at.copy_from_slice(&bytes[..8]);
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&bytes[8..]);
        Self {
            issued_at: u64::from_be_bytes(issued_at),
            signature,
        }
    }
}

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
    /// `issued_at` is the signer's clock in unix seconds; it is part of what is
    /// signed, so the certifying side can refuse a statement that has gone stale.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key cannot sign.
    pub fn signed(
        device_sk: &PrivateKey,
        account: AccountId,
        device: DeviceId,
        kem_pk: KemPublicKey,
        issued_at: u64,
    ) -> Result<(Self, PairingStatement), AccountError> {
        let offer = Self::new(account, device, kem_pk, device_sk.public_key());
        let signature = sign_payload(device_sk, &offer.payload(issued_at))?;
        Ok((
            offer,
            PairingStatement {
                issued_at,
                signature,
            },
        ))
    }

    /// Canonical bytes the pairing device signs.
    ///
    /// Covers the account it is joining, its own replica id, **both** keys the
    /// certificate will name, and the time it signed. The account is in the
    /// preimage so a statement produced for one account cannot be presented while
    /// pairing into another; the keys are there because they are the entire
    /// content of what gets certified; the time is there so the statement expires.
    #[must_use]
    pub fn payload(&self, issued_at: u64) -> [u8; 32] {
        domain_hash(
            PAIRING_STATEMENT_SIGN_DOMAIN,
            &[
                self.account.as_bytes(),
                self.device.as_bytes(),
                self.kem_pk.as_bytes(),
                AsRef::<[u8; 32]>::as_ref(&self.sign_pk),
                &issued_at.to_be_bytes(),
            ],
        )
    }

    /// Check that the party offering this key material is the party that generated
    /// it, and did so recently: that `statement` carries [`Self::sign_pk`]'s
    /// signature over exactly these four values and its issue time, and that the
    /// issue time is within [`PAIRING_STATEMENT_MAX_AGE_SECS`] before `now`.
    ///
    /// `now` is the verifier's clock in unix seconds. See the module docs for what
    /// this closes and what it does not.
    ///
    /// # Errors
    /// [`AccountError::PairingStatementInvalid`] if the signature does not verify,
    /// [`AccountError::PairingStatementExpired`] if it does but the statement is
    /// too old, or dated too far ahead of `now`.
    pub fn verify_statement(
        &self,
        statement: &PairingStatement,
        now: u64,
    ) -> Result<(), AccountError> {
        self.sign_pk
            .verify_raw_signature(&self.payload(statement.issued_at), &statement.signature)
            .map_err(|_| AccountError::PairingStatementInvalid)?;

        let too_old = now.saturating_sub(statement.issued_at) > PAIRING_STATEMENT_MAX_AGE_SECS;
        let too_new = statement.issued_at.saturating_sub(now) > PAIRING_STATEMENT_MAX_SKEW_SECS;
        if too_old || too_new {
            return Err(AccountError::PairingStatementExpired);
        }
        Ok(())
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
