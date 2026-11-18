//! Account recovery (core `docs/specs/account-recovery.md`, core #4844): a
//! person who lost every passkey gets back in with a recovery code they made
//! while signed in. The code and a new passkey start a recovery; for 72 hours
//! a passkey the account still has cancels it; after, the new passkey
//! finishes it, which ends every session the account had and signs in.
//!
//! The code is sent to the node over the sealed channel, and the node signs
//! the start with the key the code stands for (core's `proof::sign_recover`);
//! the log keeps only that key's public half. [`recovery_code_key`] derives
//! it here, byte-for-byte core's `proof::recovery_key` (pinned below and in
//! `tests/test_account_recovery.py`), so a code can be checked before it is
//! sent.

use super::prelude::*;

/// What a recovery code's key is derived from, before the canonical code
/// (core's `proof::RECOVERY_CODE_DOMAIN`).
const RECOVERY_CODE_DOMAIN: &[u8] = b"calimero.account-recovery-code.v1\0";

/// Crockford's base32 alphabet, as core's `calimero_identity` writes codes.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in a canonical code: 160 bits, 5 per character.
const CODE_CHARS: usize = 32;

/// The longest code text read, separators included (core's
/// `MAX_RECOVERY_CODE_CHARS`).
const MAX_CODE_CHARS: usize = 80;

/// A recovery code as typed back, in its canonical form, as core's
/// `calimero_identity::normalize_recovery_code` reads it: dashes and spaces
/// dropped, upper case, `O` read as `0` and `I`/`L` as `1`. `None` for
/// anything that cannot be a code.
pub(crate) fn normalize_recovery_code(code: &str) -> Option<String> {
    if code.len() > MAX_CODE_CHARS {
        return None;
    }
    let mut out = String::with_capacity(CODE_CHARS);
    for c in code.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !CROCKFORD.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == CODE_CHARS).then_some(out)
}

/// The Ed25519 key a canonical code stands for (core's
/// `proof::recovery_signing_key`).
fn recovery_signing_key(canonical: &str) -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&calimero_identity::sha256(
        &[RECOVERY_CODE_DOMAIN, canonical.as_bytes()].concat(),
    ))
}

/// A code's public key, hex, as typed (core's `recovery_key` of the
/// canonical code); `None` for what cannot be a code.
pub(crate) fn recovery_code_key(code: &str) -> Option<String> {
    normalize_recovery_code(code)
        .map(|c| hex::encode(recovery_signing_key(&c).verifying_key().to_bytes()))
}

fn not_a_code() -> PyErr {
    PyValueError::new_err(
        "a recovery code is 32 Crockford base32 characters (dashes and spaces ignored)",
    )
}

#[pymethods]
impl PyCloudClient {
    /// The canonical form of a recovery code as written down: dashes and
    /// spaces dropped, upper case, `O` read as `0` and `I`/`L` as `1`.
    /// `ValueError` for anything that cannot be a code.
    #[staticmethod]
    fn normalize_recovery_code(code: &str) -> PyResult<String> {
        normalize_recovery_code(code).ok_or_else(not_a_code)
    }

    /// The Ed25519 public key (hex) a recovery code stands for: the seed is
    /// `SHA-256("calimero.account-recovery-code.v1\0" || canonical code)`,
    /// core's `proof::recovery_key`. The node records only this; it signs a
    /// recovery's start with the private half. `ValueError` for anything that
    /// cannot be a code.
    #[staticmethod]
    fn recovery_code_key(code: &str) -> PyResult<String> {
        recovery_code_key(code).ok_or_else(not_a_code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Computed apart from this code (Python's `cryptography`) from core's
    /// definition: `SHA-256("calimero.account-recovery-code.v1\0" ||
    /// canonical code)` as the Ed25519 seed, the public key in hex.
    #[test]
    fn a_codes_key_is_cores() {
        for (canonical, seed, public) in [
            (
                "0123456789ABCDEFGHJKMNPQRSTVWXYZ",
                "48a0336318a64058b13f481631455688d318a674969703db29b857a46ab46040",
                "88736a23467015382f969b313598e1349b61839bd057843f679ab9eb92d637ef",
            ),
            (
                "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
                "3a6e865aac8ae6803c3e18b8bea46855198c8bbc1352c8fddc022f985f84a603",
                "a5a5000d6a5b9fe6c493b1d07e7266776bbd6942de0710ca48b26ab78d1bb977",
            ),
        ] {
            assert_eq!(
                hex::encode(recovery_signing_key(canonical).to_bytes()),
                seed
            );
            assert_eq!(recovery_code_key(canonical).as_deref(), Some(public));
        }
    }

    /// A code reads back however it is written down, as core's
    /// `a_recovery_code_reads_back_however_it_is_typed` reads it, and the
    /// key is the canonical code's.
    #[test]
    fn a_code_reads_back_however_it_is_typed() {
        let canonical = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        for typed in [
            "0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV-WXYZ",
            "o123 4567 89ab cdef ghjk mnpq rstv wxyz",
            "OL23456789abcdefghjkmnpqrstvwxyz",
        ] {
            assert_eq!(
                normalize_recovery_code(typed).as_deref(),
                Some(canonical),
                "{typed}"
            );
            assert_eq!(recovery_code_key(typed), recovery_code_key(canonical));
        }
    }

    /// What core's `calimero_identity` tests refuse.
    #[test]
    fn what_cannot_be_a_code_is_refused() {
        let code = "0123-4567-89AB-CDEF-GHJK-MNPQ-RSTV-WXYZ";
        assert_eq!(normalize_recovery_code("ABCD-EFGH"), None, "too short");
        assert_eq!(normalize_recovery_code(&format!("{code}U")), None);
        let padded = format!("{code}{}", " ".repeat(MAX_CODE_CHARS));
        assert_eq!(normalize_recovery_code(&padded), None, "too long as typed");
        assert_eq!(normalize_recovery_code(&code.replace('A', "é")), None);
        assert_eq!(recovery_code_key("not a code"), None);
    }
}
