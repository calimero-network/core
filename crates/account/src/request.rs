//! A signature over one HTTP request, so a caller's identity can travel with
//! the request instead of being exchanged for a session the node has to store.
//!
//! # What this is for
//!
//! The delegated surface serves callers that hold a key and have no account on
//! the node they are talking to — a browser tab, a phone, an agent. A session
//! answers "who is this" by having the node mint and remember something; this
//! answers it by having the caller prove it on every call. The node then holds
//! no session state at all, and the same code path works whether or not any
//! login provider is configured.
//!
//! # The chain this sits at the bottom of
//!
//! ```text
//! account root ──signs──▶ DeviceCert       cold, once, offline
//! device key   ──signs──▶ LoginStatement   once per session, hours
//! session key  ──signs──▶ RequestSig       per call
//! ```
//!
//! The middle link is optional. A CLI or a server-side job signs a `RequestSig`
//! with its **device** key directly and presents a two-link chain; a browser
//! keeping the device key behind a hardware or extension boundary mints a
//! session key and presents three. One verifier handles both, which is why this
//! type does not name its own signer — see [`RequestSig::verify_signature`].
//!
//! # What it binds, and why each part
//!
//! Method, path and a hash of the body. A signature for `GET /a` is therefore
//! not a signature for `POST /a`, for `GET /b`, or for the same call with
//! different arguments. Without the body, a proof captured from a harmless
//! write could be replayed with a different one; without the method, a read
//! proof would authorize the delete on the same path.
//!
//! **Replay inside the window is bounded rather than prevented.** A captured
//! signature can be presented again until it expires, and doing so performs the
//! identical request — the same read, or the same write with the same bytes.
//! That is deliberate: preventing it needs either a server-side nonce ledger
//! (state, per caller, which is what this design exists to avoid) or a
//! challenge round trip before every call. A warrant, which authorizes a state
//! change with lasting effect, keeps its nonce ledger for exactly that reason.
//! A request signature is not the place for one.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{domain_hash, PrivateKey, PublicKey};

use crate::domain::{REQUEST_BODY_DOMAIN, REQUEST_SIGN_DOMAIN};
use crate::error::AccountError;
use crate::signed::sign_payload;

/// One request, signed by the key at the bottom of a caller's chain.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Eq, PartialEq)]
pub struct RequestSig {
    /// The HTTP method, exactly as it appears on the wire.
    ///
    /// Not normalized here. `HEAD` and `GET` are the same *permission* and the
    /// permission layer treats them so, but they are different requests, and a
    /// signature layer that folded them would let a proof minted for one be
    /// presented as the other.
    pub method: String,
    /// The path, without the query string.
    ///
    /// Queries are excluded because they carry values a proxy may legitimately
    /// rewrite — a token parameter most of all — and a signature over bytes
    /// something else is entitled to change is a signature that fails for
    /// reasons the caller cannot see. Anything that must be bound belongs in
    /// the body.
    pub path: String,
    /// [`RequestSig::body_hash`] of the request body; the hash of an empty body
    /// when there is none.
    ///
    /// Always present, so there is no "absent body" case for a verifier to get
    /// wrong. An `Option` here would mean two encodings of the same request,
    /// and the one a signer chose would have to match the one a verifier
    /// assumed.
    pub body_hash: [u8; 32],
    /// Unix seconds at signing.
    pub issued_at: u64,
    /// Unix seconds after which this must not be honoured.
    ///
    /// Checked by whoever holds a clock, not here — the same division
    /// [`crate::LoginStatement`] makes, and for the same reason: this type is
    /// pure data and a clock is not.
    pub expires_at: u64,
    /// Signature over [`RequestSig::signing_payload`].
    pub signature: [u8; 64],
}

impl RequestSig {
    /// Commit to a request body.
    ///
    /// Separated from the signing domain because the two are different jobs on
    /// the same bytes: this produces the commitment, [`Self::signing_payload`]
    /// signs over it. Sharing a domain would make the commitment a truncated
    /// disclosure of bytes something signs.
    #[must_use]
    pub fn body_hash(body: &[u8]) -> [u8; 32] {
        domain_hash(REQUEST_BODY_DOMAIN, &[body])
    }

    /// The canonical bytes a signer covers.
    ///
    /// Assembled in one place so the client that mints and the node that checks
    /// cannot drift — the same reason [`crate::LoginStatement::signing_payload`]
    /// is shaped this way, and the same failure if they do: signatures that are
    /// well formed and verify nowhere, arriving as an authorization refusal far
    /// from the change that caused it.
    #[must_use]
    pub fn signing_payload(
        method: &str,
        path: &str,
        body_hash: &[u8; 32],
        issued_at: u64,
        expires_at: u64,
    ) -> [u8; 32] {
        domain_hash(
            REQUEST_SIGN_DOMAIN,
            &[
                method.as_bytes(),
                path.as_bytes(),
                body_hash,
                &issued_at.to_le_bytes(),
                &expires_at.to_le_bytes(),
            ],
        )
    }

    /// Sign one request.
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        signer_sk: &PrivateKey,
        method: &str,
        path: &str,
        body: &[u8],
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, AccountError> {
        let body_hash = Self::body_hash(body);
        let payload = Self::signing_payload(method, path, &body_hash, issued_at, expires_at);
        Ok(Self {
            method: method.to_owned(),
            path: path.to_owned(),
            body_hash,
            issued_at,
            expires_at,
            signature: sign_payload(signer_sk, &payload)?,
        })
    }

    /// The payload this signature's own fields address.
    #[must_use]
    fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            &self.method,
            &self.path,
            &self.body_hash,
            self.issued_at,
            self.expires_at,
        )
    }

    /// Check the signature against a key the **caller** supplies.
    ///
    /// The expected key is an argument rather than a field on this type, and
    /// that is the point. It comes from the link above — a
    /// [`crate::LoginStatement`]'s session key on the three-link chain, the
    /// certified device key on the two-link one — so a signature that names its
    /// own signer would introduce a second answer to a question already
    /// answered, and a verifier that checked the named one instead of the
    /// expected one would accept anything.
    ///
    /// This may be checked **before** the link above it is verified, which is
    /// the cheap-first order the surface wants: one signature rules out junk
    /// before a certificate chain is walked. Reading an unverified session key
    /// to do so is sound — tampering with it either fails here, because the
    /// attacker does not hold the matching secret, or fails when that link is
    /// verified in turn.
    ///
    /// # Errors
    /// [`AccountError::RequestSignatureInvalid`] if the signature does not
    /// verify for `signer`.
    pub fn verify_signature(&self, signer: &PublicKey) -> Result<(), AccountError> {
        signer
            .verify_raw_signature(&self.payload(), &self.signature)
            .map_err(|_ignored| AccountError::RequestSignatureInvalid)
    }

    /// Whether this signature was minted for exactly this request.
    ///
    /// Separate from [`Self::verify_signature`] for the reason
    /// [`crate::LoginStatement::addressed_to`] is separate from its own
    /// signature check: a validly signed proof presented against a different
    /// request is a client reusing a signature, while a bad signature is a
    /// forgery, and the two send whoever reads the error somewhere different.
    ///
    /// # Errors
    /// [`AccountError::RequestMismatch`] naming which part did not match.
    pub fn covers(&self, method: &str, path: &str, body: &[u8]) -> Result<(), AccountError> {
        if self.method != method {
            return Err(AccountError::RequestMismatch { part: "method" });
        }
        if self.path != path {
            return Err(AccountError::RequestMismatch { part: "path" });
        }
        if self.body_hash != Self::body_hash(body) {
            return Err(AccountError::RequestMismatch { part: "body" });
        }
        Ok(())
    }
}
