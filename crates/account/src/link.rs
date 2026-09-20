//! Proving to an outside verifier that you own an account.
//!
//! # Why this is root-signed, when almost nothing else needs to be
//!
//! Every other credential here answers a question *about a device* — which
//! account it belongs to ([`crate::DeviceCert`]), whether it is withdrawn
//! ([`crate::DeviceRevocation`]), what it may speak for ([`crate::DeviceScope`]).
//! The root signs those because a device cannot vouch for itself, and a verifier
//! then reasons about the device.
//!
//! This one asks a different question, and it is the only question a device
//! genuinely cannot answer: **is this account yours?** A device certificate is a
//! root signature, so it is tempting to offer one as the proof — but it says the
//! root certified *a device*, at some point in the past, and it is a static blob.
//! Whoever obtained a copy can present it. What an outside verifier wants before
//! it files an account under someone's name is a statement the account's own root
//! made, *to that verifier*, *now*. Only the root can make it, because the root is
//! what the account id is the content address of.
//!
//! So this is deliberately not "a device proof reused". It is the account
//! speaking for itself.
//!
//! # What each field stops
//!
//! A statement that named only the account would be a bearer token: anyone who
//! saw it could file the same account under their own login somewhere else.
//!
//! | field | stops |
//! | --- | --- |
//! | `account` | the signature being read as a claim about a different account |
//! | `audience` | a link minted for one verifier being filed by another |
//! | `challenge` | replay of an old statement against the same verifier |
//! | `issued_at` / `expires_at` | an indefinitely valid statement |
//! | `key_epoch` | a signature by a retired root key passing as a current one |
//!
//! `audience` is the same [`Audience`] a [`crate::LoginStatement`] carries, and
//! for the same reason: the variant tag is load-bearing, so a link minted for the
//! web origin `"x"` cannot be presented as one minted for the code-signing id
//! `"x"`. Reusing the type rather than taking a bare string is what keeps one
//! spelling of "who is this addressed to" across the crate.
//!
//! # Keeping the root offline
//!
//! Signing this needs the root, which is the key an account holder keeps in cold
//! storage. That is a real cost and it is the reason the statement is bounded by
//! `expires_at` and pinned to one `audience`: the root comes out once per
//! verifier, not once per session. Sessions are what [`crate::LoginStatement`] is
//! for, and it is signed by a *device* precisely so logging in never reaches for
//! this key.
//!
//! # What verification here does not settle
//!
//! [`verify_account_link`] establishes that the account's root key, at the epoch
//! the statement claims, signed these bytes. It is blind to what only the
//! verifier holds:
//!
//! * is `challenge` one this verifier issued, and unspent? — needs its own
//!   issued-set
//! * has `expires_at` passed? — needs a clock
//! * is `audience` this verifier? — needs to know its own name
//!
//! All three are the verifier's, and all three are mandatory. A caller that
//! checks only the signature has checked that the bytes are authentic and not
//! that they were meant for it.

use borsh::{BorshDeserialize, BorshSerialize};

use calimero_primitives::identity::{domain_hash, AccountId, PrivateKey};

use crate::account::AccountGenesis;
use crate::domain::ACCOUNT_LINK_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::login::Audience;
use crate::root_key::RootKeyHandoff;
use crate::signed::{sign_payload, verify_root_signed, AccountProof, RootSigned, Verified};

/// A root-signed assertion that this account is the signer's, addressed to one
/// verifier and good only until it expires.
///
/// Note what it is *not*: a [`crate::signed::DeviceBound`] statement. Nothing
/// here names a device, because a device is not what is being proved — and
/// adding one would invite a verifier to conclude something about that device's
/// standing, which needs a causal cut this credential deliberately does without.
#[derive(Clone, Debug, Eq, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct AccountLink {
    /// The account being linked.
    pub account: AccountId,
    /// Who this is addressed to. Compared byte for byte by the verifier.
    pub audience: Audience,
    /// The verifier's challenge, proving the statement is fresh.
    pub challenge: [u8; 32],
    /// Unix seconds at signing.
    pub issued_at: u64,
    /// Unix seconds after which this must not be honoured. Checked by whoever
    /// holds a clock — not here.
    pub expires_at: u64,
    /// Which account root-key epoch signed this.
    pub key_epoch: u32,
    /// Signature by the epoch-`key_epoch` root key over
    /// [`AccountLink::signing_payload`].
    pub signature: [u8; 64],
}

impl AccountLink {
    /// Canonical bytes the root key signs. Covers every field but the signature.
    ///
    /// Assembled in one place so the client that mints a link and the service
    /// that checks it cannot drift — the same rule
    /// [`crate::LoginStatement::signing_payload`] states for itself.
    #[must_use]
    pub fn signing_payload(
        account: AccountId,
        audience: &Audience,
        challenge: &[u8; 32],
        issued_at: u64,
        expires_at: u64,
        key_epoch: u32,
    ) -> [u8; 32] {
        domain_hash(
            ACCOUNT_LINK_SIGN_DOMAIN,
            &[
                account.as_bytes(),
                &audience.signing_bytes(),
                challenge,
                &issued_at.to_le_bytes(),
                &expires_at.to_le_bytes(),
                &key_epoch.to_le_bytes(),
            ],
        )
    }

    /// Mint a link for `account`, signed by its root key at `key_epoch`.
    ///
    /// The account is taken rather than derived from the key: at a rotated epoch
    /// the signing key is no longer the one the id was minted from, so deriving
    /// it would name a different account at every epoch but zero. A caller
    /// signing for an account this key does not own produces a link that fails
    /// [`verify_account_link`] against the genesis, which is where the mismatch
    /// belongs.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        root_sk: &PrivateKey,
        account: AccountId,
        audience: Audience,
        challenge: [u8; 32],
        issued_at: u64,
        expires_at: u64,
        key_epoch: u32,
    ) -> Result<Self, AccountError> {
        let payload = Self::signing_payload(
            account, &audience, &challenge, issued_at, expires_at, key_epoch,
        );
        Ok(Self {
            account,
            audience,
            challenge,
            issued_at,
            expires_at,
            key_epoch,
            signature: sign_payload(root_sk, &payload)?,
        })
    }
}

impl RootSigned for AccountLink {
    const ACCOUNT_MISMATCH: AccountError = AccountError::LinkAccountMismatch;
    const SIGNATURE_INVALID: AccountError = AccountError::LinkSignatureInvalid;

    fn account(&self) -> AccountId {
        self.account
    }

    fn key_epoch(&self) -> u32 {
        self.key_epoch
    }

    fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            self.account,
            &self.audience,
            &self.challenge,
            self.issued_at,
            self.expires_at,
            self.key_epoch,
        )
    }

    fn signature(&self) -> &[u8; 64] {
        &self.signature
    }
}

/// An [`AccountLink`] together with everything needed to verify it.
///
/// This is the wire form, and the reason an outside verifier needs nothing from
/// Calimero to check one: the genesis hashes to the [`AccountId`], and the chain
/// carries the rotations between that and the signing epoch. No node to ask, no
/// membership to resolve, no causal cut.
pub type SignedAccountLink = AccountProof<AccountLink>;

/// An [`AccountLink`] whose anchor, chain, and signature have all been checked.
/// See [`Verified`] for what that does — and does not — mean.
pub type VerifiedAccountLink = Verified<AccountLink>;

/// Verify a link against the account it names, from the account id alone.
///
/// `claimed_account` is what ties the credential to something outside it — for a
/// verifier filing a link, the account it is about to file. Without it a caller
/// would happily verify a well-formed link for an account nobody asked about.
///
/// **This does not make the link honourable.** The audience, the challenge and
/// the expiry are the verifier's to check, and it must: see the module docs.
///
/// # Errors
/// See [`verify_root_signed`].
pub fn verify_account_link(
    claimed_account: AccountId,
    genesis: &AccountGenesis,
    chain: &[RootKeyHandoff],
    link: &AccountLink,
) -> Result<VerifiedAccountLink, AccountError> {
    verify_root_signed(claimed_account, genesis, chain, link)?;
    Ok(Verified::new(link.clone()))
}
