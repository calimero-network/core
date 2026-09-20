//! Root signatures for verifiers that define their own wire format.
//!
//! Everything else in this crate is a statement *core* designed: a borsh struct
//! over a core domain, verified by [`crate::verify_root_signed`]. This is the
//! other direction — an outside verifier already specified what the account root
//! must sign, it shipped before we got here, and core's job is to produce those
//! exact bytes rather than to propose a format of its own.
//!
//! mdma is the case in hand. Its manager verifies
//! `ed25519(root, DOMAIN ‖ nonce_utf8)` for three separate purposes, against a
//! nonce it sealed itself. A node holding the root is the only thing that can
//! sign it, since the root deliberately never leaves the store.
//!
//! **Why an allowlist rather than arbitrary bytes.** "Sign these bytes with the
//! account root" is a signing oracle, and the root is the one key that can
//! certify a device — that is, take over the account. A length guard is not
//! enough to make it safe: core's own credentials sign a 32-byte
//! `domain_hash` digest, so refusing 32-byte payloads looks sufficient, but
//! `calimero_governance_types::admitter_endorsement_payload` signs a raw
//! concatenation of variable length, and nothing stops a future signing site
//! from doing the same. A guard that depends on auditing every signing site in
//! the workspace forever is the wrong shape. Naming the reachable domains
//! inverts it: a new signing site elsewhere cannot become a target here, because
//! its domain is not in [`ExternalSigningDomain`].
//!
//! The caller still owns the payload, which is the whole point — mdma's payload
//! *is* its nonce, so a client that knows the verifier's format can produce it
//! without core learning anything about that format beyond the domain.

use calimero_primitives::identity::{PrivateKey, PublicKey};

use crate::error::AccountError;

/// A domain an outside verifier defined, which this account's root may sign under.
///
/// Deliberately a closed enum rather than a string: the set of things the root
/// will sign for a non-core verifier is a security boundary, and it should take
/// a code review to widen it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ExternalSigningDomain {
    /// Bind a Calimero account to an mdma cloud login.
    MdmaAccountLink,
    /// Open an mdma session as the account.
    MdmaAccountLogin,
    /// Read recovery material from mdma while the device is gone.
    MdmaAccountRecovery,
}

impl ExternalSigningDomain {
    /// The exact bytes the verifier prepends, trailing NUL included.
    ///
    /// Byte-for-byte what mdma's `manager/app/account_proof.py` declares. These
    /// are *its* constants mirrored here, not ours to renumber: a `.v2` appears
    /// here only after it appears there.
    ///
    /// The trailing `\0` is part of each one, and is what separates the domain
    /// from the payload in a plain concatenation — mdma does not length-prefix
    /// the way core's `domain_hash` does, so the separator has to live in the
    /// constant.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::MdmaAccountLink => b"calimero.mdma.account-link.v1\0",
            Self::MdmaAccountLogin => b"calimero.mdma.account-login.v1\0",
            Self::MdmaAccountRecovery => b"calimero.mdma.account-recovery.v1\0",
        }
    }

    /// The wire name a caller asks for, as the API and CLI spell it.
    ///
    /// Short names rather than the raw domain bytes: a caller that could send
    /// bytes could send *any* bytes, which is the oracle this type exists to
    /// prevent. The mapping is one-way on purpose.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "mdma.account-link" => Some(Self::MdmaAccountLink),
            "mdma.account-login" => Some(Self::MdmaAccountLogin),
            "mdma.account-recovery" => Some(Self::MdmaAccountRecovery),
            _ => None,
        }
    }

    /// Every name [`Self::from_name`] accepts, for error messages and `--help`.
    #[must_use]
    pub const fn names() -> &'static [&'static str] {
        &[
            "mdma.account-link",
            "mdma.account-login",
            "mdma.account-recovery",
        ]
    }
}

/// Sign `domain ‖ payload` with the account root, for an outside verifier.
///
/// A plain concatenation signed directly — **not** [`crate::verify_root_signed`]'s
/// shape and not a `domain_hash` digest. That asymmetry is the point: the
/// verifier specified this, and a signature core finds tidier is a signature
/// that does not verify.
///
/// Returns the public key alongside, because every consumer needs both and
/// deriving one from a secret the caller does not hold is not something they can
/// do themselves.
pub fn sign_external(
    root_sk: &PrivateKey,
    domain: ExternalSigningDomain,
    payload: &[u8],
) -> Result<(PublicKey, [u8; 64]), AccountError> {
    let domain_bytes = domain.as_bytes();

    let mut message = Vec::with_capacity(domain_bytes.len() + payload.len());
    message.extend_from_slice(domain_bytes);
    message.extend_from_slice(payload);

    let signature = root_sk
        .sign(&message)
        .map_err(|_| AccountError::SigningFailed)?
        .to_bytes();

    Ok((root_sk.public_key(), signature))
}
