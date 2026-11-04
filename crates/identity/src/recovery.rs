//! An organization's recovery key: an Ed25519 key whose 32-byte seed the
//! owner keeps offline. The node stores only the public half; the seed signs
//! one statement, [`recovery_message`], which hands the organization to one
//! account. Clients sign here (or with the same bytes in mero-js and
//! calimero-client-py); the node only verifies.

use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};

use crate::{b64url, from_b64url, random, Error};

/// What a recovery signature covers before the organization, the account and
/// the nonce.
pub const RECOVERY_DOMAIN: &[u8] = b"calimero.org-recovery.v1\0";

/// The longest nonce a recovery carries, in bytes.
pub const MAX_RECOVERY_NONCE_BYTES: usize = 64;

/// The bytes a recovery signs: `"calimero.org-recovery.v1\0" || org || 0 ||
/// account || 0 || nonce`.
#[must_use]
pub fn recovery_message(org: &str, account: &str, nonce: &str) -> Vec<u8> {
    let mut m = RECOVERY_DOMAIN.to_vec();
    m.extend_from_slice(org.as_bytes());
    m.push(0);
    m.extend_from_slice(account.as_bytes());
    m.push(0);
    m.extend_from_slice(nonce.as_bytes());
    m
}

/// A new random seed.
#[must_use]
pub fn new_recovery_seed() -> [u8; 32] {
    random()
}

/// The seed's public key, base64url: what the organization stores.
#[must_use]
pub fn recovery_public(seed: &[u8; 32]) -> String {
    b64url(key_pair(seed).public_key().as_ref())
}

/// The seed's signature (base64url) handing `org` to `account`.
#[must_use]
pub fn sign_recovery(seed: &[u8; 32], org: &str, account: &str, nonce: &str) -> String {
    b64url(
        key_pair(seed)
            .sign(&recovery_message(org, account, nonce))
            .as_ref(),
    )
}

/// Check that `signature` is the recovery key `public`'s, handing `org` to
/// `account` under `nonce`.
///
/// # Errors
/// A malformed key, nonce or signature, or one the key did not make.
pub fn verify_recovery(
    public: &str,
    org: &str,
    account: &str,
    nonce: &str,
    signature: &str,
) -> Result<(), Error> {
    if nonce.is_empty() || nonce.len() > MAX_RECOVERY_NONCE_BYTES {
        return Err(Error::Malformed("recovery nonce"));
    }
    let public = parse_public(public)?;
    let signature = from_b64url(signature).map_err(|_| Error::Malformed("recovery signature"))?;
    if signature.len() != 64 {
        return Err(Error::Malformed("recovery signature"));
    }
    UnparsedPublicKey::new(&ED25519, &public)
        .verify(&recovery_message(org, account, nonce), &signature)
        .map_err(|_| Error::BadSignature)
}

/// A recovery public key (base64url of 32 bytes), decoded.
///
/// # Errors
/// Not base64url, or not 32 bytes.
pub fn parse_public(public: &str) -> Result<[u8; 32], Error> {
    from_b64url(public)
        .ok()
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .ok_or(Error::Malformed("recovery key"))
}

/// The seed as people write it down: 64 hex digits in eight groups of eight,
/// joined by `-`.
#[must_use]
pub fn seed_text(seed: &[u8; 32]) -> String {
    seed.chunks(4)
        .map(|c| c.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

/// A seed from its written form: 64 hex digits; `-` and whitespace are
/// ignored, case too.
///
/// # Errors
/// Anything but 64 hex digits.
pub fn parse_seed(text: &str) -> Result<[u8; 32], Error> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b'-' && !b.is_ascii_whitespace())
        .collect();
    if digits.len() != 64 {
        return Err(Error::Malformed("recovery seed"));
    }
    let nibble = |d: u8| {
        (d as char)
            .to_digit(16)
            .ok_or(Error::Malformed("recovery seed"))
    };
    let mut seed = [0_u8; 32];
    for (i, pair) in digits.chunks(2).enumerate() {
        let hi = nibble(pair[0])?;
        let lo = nibble(pair[1])?;
        seed[i] = u8::try_from(hi * 16 + lo).map_err(|_| Error::Malformed("recovery seed"))?;
    }
    Ok(seed)
}

fn key_pair(seed: &[u8; 32]) -> Ed25519KeyPair {
    // SAFETY: any 32 bytes are an Ed25519 seed; `from_seed_unchecked` fails
    // only on a wrong length, which the array type rules out.
    Ed25519KeyPair::from_seed_unchecked(seed).expect("a 32-byte seed is an Ed25519 key")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_signs_for_one_org_account_and_nonce() {
        let seed = [7_u8; 32];
        let public = recovery_public(&seed);
        let sig = sign_recovery(&seed, "org", "bob", "n1");
        assert_eq!(verify_recovery(&public, "org", "bob", "n1", &sig), Ok(()));
        for (org, account, nonce) in [
            ("other", "bob", "n1"),
            ("org", "eve", "n1"),
            ("org", "bob", "n2"),
        ] {
            assert_eq!(
                verify_recovery(&public, org, account, nonce, &sig),
                Err(Error::BadSignature)
            );
        }
        let other = recovery_public(&[8_u8; 32]);
        assert_eq!(
            verify_recovery(&other, "org", "bob", "n1", &sig),
            Err(Error::BadSignature)
        );
        assert!(verify_recovery(&public, "org", "bob", "", &sig).is_err());
        let long = "n".repeat(MAX_RECOVERY_NONCE_BYTES + 1);
        assert!(verify_recovery(&public, "org", "bob", &long, &sig).is_err());
        assert!(verify_recovery("short", "org", "bob", "n1", &sig).is_err());
    }

    #[test]
    fn a_seed_reads_back_from_its_written_form() {
        let seed: [u8; 32] = core::array::from_fn(|i| u8::try_from(i * 7).unwrap_or(0));
        let text = seed_text(&seed);
        assert_eq!(text.len(), 64 + 7);
        assert_eq!(parse_seed(&text), Ok(seed));
        assert_eq!(parse_seed(&text.to_uppercase().replace('-', " ")), Ok(seed));
        assert!(parse_seed(&text[..70]).is_err());
        assert!(parse_seed(&text.replace('0', "g")).is_err());
    }

    /// The bytes mero-js and calimero-client-py must produce for the same
    /// seed (`[1; 32]`): the public key and a signature, pinned.
    #[test]
    fn the_wire_bytes_are_pinned_for_the_clients() {
        let seed = [1_u8; 32];
        assert_eq!(
            recovery_message("o", "a", "n"),
            b"calimero.org-recovery.v1\0o\0a\0n".to_vec()
        );
        assert_eq!(
            recovery_public(&seed),
            "iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w"
        );
        assert_eq!(
            sign_recovery(&seed, "o", "a", "n"),
            "bbbHMaH2SC50zQADskIDS5xg8WxZFrtxdasJzhzCOMZh-aBrUxeDKro2_UUIyU4PzH5LmsS1gQRQmVfx3E1eDw"
        );
    }
}
