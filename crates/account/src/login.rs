//! The statement a device key signs to obtain a session on a node.
//!
//! # Why it is shaped this way
//!
//! **It is signed by a device key, like a [`crate::Warrant`] and unlike every
//! root-signed credential here.** Obtaining a session must not require the key
//! that mints devices — that key is the one thing an account holder keeps
//! offline, and reaching for it to log in would defeat the reason it is offline.
//! So this is not an [`crate::AccountProof`]; it travels *beside* one. The proof
//! says which account a device belongs to, and this says what that device is
//! asking for.
//!
//! **Every field exists to stop a signature being useful somewhere it was not
//! meant for.** A statement that named only the challenge would be a bearer
//! token for whoever collected it:
//!
//! | field | stops |
//! | --- | --- |
//! | `node` | replaying the session against a different node |
//! | `audience` | a session minted for one client surface being used from another |
//! | `challenge` | replay of an old statement against the same node |
//! | `session_key` | the statement authorizing a key its signer did not choose |
//! | `issued_at` / `expires_at` | an indefinitely valid statement |
//!
//! **`node` is a [`PublicKey`], not a name.** The client must learn the node's
//! identity from something it pinned, never from the challenge response — an
//! attacker who can answer on the node's behalf would otherwise choose what the
//! device signs about, which is the whole attack this field exists to stop. A
//! key is the only form of that identity a client can pin and a verifier can
//! compare without a lookup.
//!
//! **The signature covers a session key it does not hold.** `session_key` is
//! minted by the client per session and thrown away with it, so the device key
//! signs once and the short-lived key does the talking. That is what keeps a
//! device key off the wire for the life of a session.
//!
//! # What verification here does not settle
//!
//! [`LoginStatement::verify_signature`] establishes only that the named device
//! key signed these bytes. It is blind to everything a bundle cannot see:
//!
//! * does `device_key` belong to the account? — that is the accompanying
//!   [`crate::AccountProof`], checked by the caller
//! * has that device been **revoked**? — needs a causal cut
//! * is the `challenge` one this node issued, and unspent? — needs the node's
//!   own MAC key and spent-set
//! * has `expires_at` passed? — needs a clock
//!
//! The first belongs to the caller, the second to `calimero-authz`, and the last
//! two to the authentication service, which is the only party holding both.

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_primitives::identity::{domain_hash, PrivateKey, PublicKey};

use crate::domain::AUTH_LOGIN_SIGN_DOMAIN;
use crate::error::AccountError;
use crate::signed::sign_payload;

/// The client surface a session is minted for.
///
/// A session is bound to one of these so that a token obtained by a web page
/// cannot be presented by a native client, or the reverse. Without it, a single
/// compromised surface yields sessions usable from every other one.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Eq, PartialEq)]
pub enum Audience {
    /// A browser origin, exactly as the browser spells it (`https://host:port`,
    /// no trailing slash). Compared byte for byte — normalizing here would mean
    /// two spellings of one origin, and a verifier disagreeing with the browser
    /// about which one a token was for.
    WebOrigin(String),
    /// A signed native client, named by its code-signing identity.
    CodeSigningId(String),
    /// A command-line client, which has no origin and no signing identity to
    /// name. Deliberately carries no payload: an attacker-chosen string here
    /// would be an audience that binds nothing while looking like it binds
    /// something.
    Cli,
}

impl Audience {
    /// Map a caller's spelling onto the variant it names.
    ///
    /// One copy, because two would be a drift nobody sees until a statement
    /// minted by one client is refused for another: `merod` spells an audience on
    /// a command line and the admin API spells it in JSON, and both must land on
    /// the same variant for the same string or a signature binds a surface its
    /// holder did not mean.
    ///
    /// A web origin is passed through verbatim rather than normalized. The
    /// verifier compares it byte for byte against what the browser sends, so
    /// "helpfully" stripping a trailing slash here would produce a statement the
    /// browser's own origin no longer matches.
    #[must_use]
    pub fn from_spelling(spelling: &str) -> Self {
        match spelling.trim() {
            "cli" => Self::Cli,
            other if other.starts_with("http://") || other.starts_with("https://") => {
                Self::WebOrigin(other.to_owned())
            }
            other => Self::CodeSigningId(other.to_owned()),
        }
    }

    /// The bytes this variant contributes to the signing payload.
    ///
    /// Tag-prefixed so `WebOrigin("x")` and `CodeSigningId("x")` cannot produce
    /// the same preimage — without the tag, one audience could be presented as
    /// the other, which is exactly what the field exists to prevent.
    fn signing_bytes(&self) -> Vec<u8> {
        let (tag, body): (u8, &[u8]) = match self {
            Self::WebOrigin(origin) => (0, origin.as_bytes()),
            Self::CodeSigningId(id) => (1, id.as_bytes()),
            Self::Cli => (2, b""),
        };
        let mut out = Vec::with_capacity(1 + body.len());
        out.push(tag);
        out.extend_from_slice(body);
        out
    }
}

/// A device key's request for a session on one node, for one client surface.
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, Eq, PartialEq)]
pub struct LoginStatement {
    /// The node this session is for, as the key a client pins.
    pub node: PublicKey,
    /// The client surface the session is bound to.
    pub audience: Audience,
    /// The challenge this node issued, proving the statement is fresh.
    pub challenge: [u8; 32],
    /// The ephemeral key the session will actually speak with.
    pub session_key: PublicKey,
    /// The device key that signed this. Named rather than inferred so a verifier
    /// knows which key to check before it has resolved the account.
    pub device_key: PublicKey,
    /// Unix seconds at signing.
    pub issued_at: u64,
    /// Unix seconds after which this statement must not be honoured. Checked by
    /// whoever holds a clock — not here.
    pub expires_at: u64,
    /// Signature by [`Self::device_key`] over [`Self::signing_payload`].
    pub signature: [u8; 64],
}

impl LoginStatement {
    /// The canonical bytes a device signs.
    ///
    /// Assembled in one place so the client that mints a statement and the
    /// service that checks it cannot drift. Covers every field but the signature.
    #[must_use]
    pub fn signing_payload(
        node: &PublicKey,
        audience: &Audience,
        challenge: &[u8; 32],
        session_key: &PublicKey,
        device_key: &PublicKey,
        issued_at: u64,
        expires_at: u64,
    ) -> [u8; 32] {
        domain_hash(
            AUTH_LOGIN_SIGN_DOMAIN,
            &[
                AsRef::<[u8; 32]>::as_ref(node),
                &audience.signing_bytes(),
                challenge,
                AsRef::<[u8; 32]>::as_ref(session_key),
                AsRef::<[u8; 32]>::as_ref(device_key),
                &issued_at.to_le_bytes(),
                &expires_at.to_le_bytes(),
            ],
        )
    }

    /// Mint a statement, signed by the device key.
    ///
    /// `device_key` is derived from `device_sk` rather than taken as an argument,
    /// for the reason [`crate::Warrant::sign`] does the same: a caller able to
    /// name a key it does not hold could produce a statement it cannot sign, and
    /// the field would stop meaning "who asked for this session".
    ///
    /// # Errors
    /// [`AccountError::SigningFailed`] if the key refuses to sign.
    pub fn sign(
        device_sk: &PrivateKey,
        node: PublicKey,
        audience: Audience,
        challenge: [u8; 32],
        session_key: PublicKey,
        issued_at: u64,
        expires_at: u64,
    ) -> Result<Self, AccountError> {
        let device_key = device_sk.public_key();
        let payload = Self::signing_payload(
            &node,
            &audience,
            &challenge,
            &session_key,
            &device_key,
            issued_at,
            expires_at,
        );
        Ok(Self {
            node,
            audience,
            challenge,
            session_key,
            device_key,
            issued_at,
            expires_at,
            signature: sign_payload(device_sk, &payload)?,
        })
    }

    /// The payload this statement's own fields address.
    #[must_use]
    fn payload(&self) -> [u8; 32] {
        Self::signing_payload(
            &self.node,
            &self.audience,
            &self.challenge,
            &self.session_key,
            &self.device_key,
            self.issued_at,
            self.expires_at,
        )
    }

    /// Check the signature against the device key the statement names.
    ///
    /// Establishes that whoever holds `device_key` produced this statement. It
    /// says nothing about whether that key speaks for any account — the
    /// accompanying [`crate::AccountProof`] is what ties the two together, and
    /// the caller must check both.
    ///
    /// # Errors
    /// [`AccountError::LoginSignatureInvalid`] if the signature does not verify.
    pub fn verify_signature(&self) -> Result<(), AccountError> {
        self.device_key
            .verify_raw_signature(&self.payload(), &self.signature)
            .map_err(|_ignored| AccountError::LoginSignatureInvalid)
    }

    /// Whether this statement was minted for `node` and `audience`.
    ///
    /// Separate from [`Self::verify_signature`] because a validly signed
    /// statement presented to the wrong node, or from the wrong surface, is a
    /// different failure from a forged one and sends whoever reads the error
    /// somewhere different.
    ///
    /// # Errors
    /// [`AccountError::LoginNodeMismatch`] or [`AccountError::LoginAudienceMismatch`].
    pub fn addressed_to(&self, node: &PublicKey, audience: &Audience) -> Result<(), AccountError> {
        if self.node != *node {
            return Err(AccountError::LoginNodeMismatch);
        }
        if self.audience != *audience {
            return Err(AccountError::LoginAudienceMismatch);
        }
        Ok(())
    }
}
