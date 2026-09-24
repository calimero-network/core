//! The whole chain a caller presents on one request, and what checking it
//! establishes.
//!
//! # What this answers, and what it deliberately does not
//!
//! Verifying a [`CallerProof`] establishes exactly one thing: **which account
//! made this request**. It is pure cryptography over self-contained bytes — no
//! store, no network, no prior relationship with the caller — which is what
//! lets the same code serve a desktop node, a peer's self-hosted node and a
//! hosted relay without any of them being configured for it.
//!
//! It says nothing about what that account may *have*. Membership is a position
//! in a causal log, revocation is a governance row, and both are answered
//! per request against live state by whoever holds it. [`Verified`] already
//! makes the same division and says so:
//!
//! > Holding one means the credential is **internally** valid. It does **not**
//! > mean the statement is currently in force: whether the signing epoch has
//! > since been superseded, whether the device was revoked, and whether the
//! > account is even a member […]
//!
//! [`VerifiedCaller`] therefore carries the device id as well as the account,
//! because the revocation check downstream needs it and cannot recover it from
//! the account alone.
//!
//! # Order of checks
//!
//! Cheap before expensive, and each failure costs strictly less than the next
//! step would have:
//!
//! ```text
//! request covers this method/path/body    no crypto
//! freshness, both links                   no crypto
//! statement addressed to this node        no crypto
//! request signature                       1 signature
//! session statement signature             1 signature
//! account proof chain                     n signatures
//! ```
//!
//! An attacker can mint fresh, correctly addressed junk and force the first
//! signature check; they cannot make one junk request cost a certificate-chain
//! walk. That ordering is the whole reason the request signature is verified
//! against an *unverified* session key — see [`RequestSig::verify_signature`],
//! which explains why reading it early is sound.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{AccountId, DeviceId, PublicKey};

use crate::device::DeviceCert;
use crate::error::AccountError;
use crate::login::{Audience, LoginStatement};
use crate::request::RequestSig;
use crate::signed::AccountProof;

/// Everything a caller sends to prove who it is.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Eq, PartialEq)]
pub struct CallerProof {
    /// The account's certificate for the device at the top of this chain.
    pub account_proof: AccountProof<DeviceCert>,
    /// The device's delegation to an ephemeral session key.
    ///
    /// Absent on the short chain, where the device key signs each request
    /// itself. A CLI or a server-side job has its device key to hand and gains
    /// nothing from the extra link; a browser keeping that key behind a
    /// hardware or extension boundary crosses it once per session instead of
    /// once per call.
    pub session: Option<LoginStatement>,
    /// The signature over this particular request.
    pub request: RequestSig,
}

/// Who a verified chain says is calling.
///
/// Cryptographically sound and **not** authorized: see the module docs. Nothing
/// here has consulted governance, so a revoked device and a removed member both
/// produce one of these.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedCaller {
    /// The account the request speaks for.
    pub account: AccountId,
    /// The device whose key is at the top of the chain.
    ///
    /// Carried because revocation is per device and per group, so the check
    /// that consults it happens where the group is known — not here.
    pub device: DeviceId,
    /// The client surface a session was minted for, when there was a session.
    ///
    /// Returned rather than judged. Which audiences a node serves is operator
    /// policy read from configuration, and this crate has none; deciding it
    /// here would bake a deployment's policy into a signature check.
    pub audience: Option<Audience>,
}

impl CallerProof {
    /// The key that must have signed the request: the session key when a
    /// session is present, the certified device key otherwise.
    const fn expected_request_signer<'a>(&'a self, cert: &'a DeviceCert) -> &'a PublicKey {
        match &self.session {
            Some(session) => &session.session_key,
            None => &cert.sign_pk,
        }
    }

    /// Whether a timestamped window is open at `now`, allowing for clock skew.
    fn fresh(
        issued_at: u64,
        expires_at: u64,
        now: u64,
        skew: u64,
        part: &'static str,
    ) -> Result<(), AccountError> {
        if now.saturating_add(skew) < issued_at {
            return Err(AccountError::ProofNotYetValid { part });
        }
        if now.saturating_sub(skew) > expires_at {
            return Err(AccountError::ProofExpired { part });
        }
        Ok(())
    }

    /// Check the whole chain against one request.
    ///
    /// `node` is this node's own device signing key. It is what makes a proof
    /// minted for one node unusable at another, and on the short chain there is
    /// nothing to bind — a device-signed request carries no node field, so a
    /// deployment relying on that binding must require the session link. That
    /// asymmetry is deliberate and is why [`VerifiedCaller::audience`] is an
    /// `Option` rather than a default: both say "this chain had no session",
    /// and a caller that cares must look.
    ///
    /// `skew` is the tolerance applied to both windows. A client whose clock
    /// runs fast otherwise mints proofs that are not yet valid, and one running
    /// slow mints proofs already expired — neither of which is a security
    /// property, and both of which read to a user as "the login is broken".
    ///
    /// # Errors
    /// The first check that fails, named so a reader is sent to the right
    /// place: a request that does not match, a window that is closed, a
    /// statement for another node, a bad signature at any link, or a session
    /// delegating from a device the certificate does not cover.
    pub fn verify(
        &self,
        node: &PublicKey,
        method: &str,
        path: &str,
        body: &[u8],
        now: u64,
        skew: u64,
    ) -> Result<VerifiedCaller, AccountError> {
        // 1. Is this even the request that was signed? Pure comparison, and it
        //    rules out every reused proof before any key is touched.
        self.request.covers(method, path, body)?;

        Self::fresh(
            self.request.issued_at,
            self.request.expires_at,
            now,
            skew,
            "request",
        )?;

        if let Some(session) = &self.session {
            Self::fresh(session.issued_at, session.expires_at, now, skew, "session")?;
            // Addressed here, not merely valid. Without this a hostile relay
            // could take a statement a user signed for it and present it to us.
            if session.node != *node {
                return Err(AccountError::LoginNodeMismatch);
            }
        }

        // 2. One signature, against the key the chain names. On the long chain
        //    that key has not been verified yet, which is sound: tampering with
        //    it either fails here — the attacker holds no matching secret — or
        //    fails at step 3 when the statement itself is checked.
        let cert = &self.account_proof.statement;
        self.request
            .verify_signature(self.expected_request_signer(cert))?;

        // 3. The session is the device's, and the device is the certificate's.
        if let Some(session) = &self.session {
            session.verify_signature()?;
            if session.device_key != cert.sign_pk {
                return Err(AccountError::SessionDeviceMismatch);
            }
        }

        // 4. The expensive one last: the certificate chain back to a genesis
        //    whose hash IS the account id, so naming an account is not a free
        //    choice.
        let account = cert.account;
        let verified = self.account_proof.verify(account)?;

        Ok(VerifiedCaller {
            account,
            device: verified.get().device,
            audience: self.session.as_ref().map(|s| s.audience.clone()),
        })
    }
}
