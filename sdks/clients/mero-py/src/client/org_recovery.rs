//! Organization recovery (core `docs/specs/org-recovery.md`): an
//! organization's recovery key is an Ed25519 key whose seed the owner keeps
//! offline; the node stores the public half. The seed signs here, never on a
//! node, byte-for-byte with core's `calimero_identity::recovery` (pinned in
//! `tests/test_org_recovery.py`).

use ed25519_dalek::{Signer, SigningKey};

use super::prelude::*;

/// What a recovery signature covers before the organization, the account and
/// the nonce.
const RECOVERY_DOMAIN: &[u8] = b"calimero.org-recovery.v1\0";

/// The bytes a recovery signs: `"calimero.org-recovery.v1\0" || org || 0 ||
/// account || 0 || nonce`.
fn recovery_message(org: &str, account: &str, nonce: &str) -> Vec<u8> {
    let mut m = RECOVERY_DOMAIN.to_vec();
    m.extend_from_slice(org.as_bytes());
    m.push(0);
    m.extend_from_slice(account.as_bytes());
    m.push(0);
    m.extend_from_slice(nonce.as_bytes());
    m
}

/// A seed from its written form: 64 hex digits; `-`, whitespace and case are
/// ignored.
fn parse_seed(text: &str) -> PyResult<SigningKey> {
    let digits: String = text
        .chars()
        .filter(|c| *c != '-' && !c.is_whitespace())
        .collect();
    let seed: [u8; 32] = hex::decode(digits)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or_else(|| PyValueError::new_err("a recovery seed is 64 hex digits"))?;
    Ok(SigningKey::from_bytes(&seed))
}

/// The seed as people write it down: eight groups of eight hex digits.
fn seed_text(seed: &[u8; 32]) -> String {
    seed.chunks(4)
        .map(hex::encode)
        .collect::<Vec<_>>()
        .join("-")
}

#[pymethods]
impl PyCloudClient {
    /// A new recovery seed, as the owner writes it down (64 hex digits in
    /// groups of eight). Keep it offline; give the organization only
    /// `recovery_public_key(seed)`.
    #[staticmethod]
    fn new_recovery_seed() -> String {
        seed_text(&calimero_identity::random::<32>())
    }

    /// The public key (base64url) of a recovery seed: what the organization
    /// stores (`create_org(.., recovery_key=)`, `set_recovery_key`).
    #[staticmethod]
    fn recovery_public_key(seed: &str) -> PyResult<String> {
        Ok(calimero_identity::b64url(
            parse_seed(seed)?.verifying_key().as_bytes(),
        ))
    }

    /// The seed's signature (base64url) handing `org` to `account` under
    /// `nonce`, as `start_recovery` sends it.
    #[staticmethod]
    fn sign_recovery(seed: &str, org: &str, account: &str, nonce: &str) -> PyResult<String> {
        if nonce.is_empty() || nonce.len() > 64 {
            return Err(PyValueError::new_err("a recovery nonce is 1 to 64 bytes"));
        }
        let sig = parse_seed(seed)?.sign(&recovery_message(org, account, nonce));
        Ok(calimero_identity::b64url(&sig.to_bytes()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vectors core pins in
    /// `calimero_identity::recovery::the_wire_bytes_are_pinned_for_the_clients`.
    #[test]
    fn the_wire_bytes_match_core() {
        let seed = "01".repeat(32);
        assert_eq!(
            PyCloudClient::recovery_public_key(&seed).unwrap(),
            "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w"
        );
        assert_eq!(
            PyCloudClient::sign_recovery(&seed, "o", "a", "n").unwrap(),
            "bbbHMaH2SC50zQADskIDS5xg8WxZFrtxdasJzhzCOMZh-aBrUxeDKro2_UUIyU4PzH5LmsS1gQRQmVfx3E1eDw"
        );
        let s: [u8; 32] = core::array::from_fn(|i| u8::try_from(i * 7).unwrap_or(0));
        let text = seed_text(&s);
        assert_eq!(text.len(), 71);
        assert_eq!(parse_seed(&text.to_uppercase()).unwrap().to_bytes(), s);
        assert!(parse_seed(&text[..70]).is_err());
    }
}
