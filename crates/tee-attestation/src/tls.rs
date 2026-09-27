//! The TLS key a TD serves, as [`attest_tls_binding`](crate::attest_tls_binding)
//! binds it.

use sha2::{Digest, Sha256};
use x509_parser::pem::Pem;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::AttestationError;

/// SHA-256 of a DER certificate's `SubjectPublicKeyInfo`: the key, with its
/// algorithm, and nothing that changes when the certificate is renewed.
///
/// The same value HTTP public-key pinning used (`pin-sha256`), so a pin taken
/// with ordinary tools (`openssl x509 -pubkey | openssl pkey -pubin -outform
/// der | sha256sum`) matches it.
///
/// # Errors
/// When `der` is not an X.509 certificate.
pub fn tls_spki_sha256(der: &[u8]) -> Result<[u8; 32], AttestationError> {
    let (_rest, certificate) = X509Certificate::from_der(der)
        .map_err(|err| AttestationError::InvalidCertificate(err.to_string()))?;
    Ok(Sha256::digest(certificate.public_key().raw).into())
}

/// [`tls_spki_sha256`] of the first certificate in a PEM file: the leaf, in the
/// order a server's chain file lists it.
///
/// # Errors
/// When `pem` holds no certificate, or its first one does not parse.
pub fn tls_spki_sha256_from_pem(pem: &[u8]) -> Result<[u8; 32], AttestationError> {
    for block in Pem::iter_from_buffer(pem) {
        let block = block.map_err(|err| AttestationError::InvalidCertificate(err.to_string()))?;
        if block.label == "CERTIFICATE" {
            return tls_spki_sha256(&block.contents);
        }
    }
    Err(AttestationError::InvalidCertificate(
        "no CERTIFICATE block in the PEM".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{tls_spki_sha256, tls_spki_sha256_from_pem};

    /// A self-signed P-256 certificate, like the placeholder mero-tee's image
    /// generates, and its SPKI digest as `openssl` computes it:
    /// `openssl x509 -pubkey -noout | openssl pkey -pubin -outform der | sha256sum`.
    const CERTIFICATE: &str = include_str!("../tests/fixtures/tls-cert.pem");
    const SPKI_SHA256: &str = include_str!("../tests/fixtures/tls-cert.spki-sha256");

    #[test]
    fn the_spki_digest_matches_openssl() {
        let digest = tls_spki_sha256_from_pem(CERTIFICATE.as_bytes()).unwrap();
        assert_eq!(hex::encode(digest), SPKI_SHA256.trim());
    }

    #[test]
    fn a_chain_is_pinned_by_its_leaf() {
        let chain = format!("{CERTIFICATE}{CERTIFICATE_OTHER}");
        assert_eq!(
            tls_spki_sha256_from_pem(chain.as_bytes()).unwrap(),
            tls_spki_sha256_from_pem(CERTIFICATE.as_bytes()).unwrap()
        );
    }

    #[test]
    fn a_renewed_certificate_keeps_its_digest() {
        // Same key, reissued: what a Let's Encrypt renewal of the node's key is.
        assert_eq!(
            tls_spki_sha256_from_pem(CERTIFICATE_RENEWED.as_bytes()).unwrap(),
            tls_spki_sha256_from_pem(CERTIFICATE.as_bytes()).unwrap()
        );
        assert_ne!(
            tls_spki_sha256_from_pem(CERTIFICATE_OTHER.as_bytes()).unwrap(),
            tls_spki_sha256_from_pem(CERTIFICATE.as_bytes()).unwrap()
        );
    }

    #[test]
    fn what_is_not_a_certificate_is_refused() {
        assert!(tls_spki_sha256(b"not der").is_err());
        assert!(tls_spki_sha256_from_pem(b"").is_err());
        assert!(tls_spki_sha256_from_pem(KEY_ONLY.as_bytes()).is_err());
    }

    const CERTIFICATE_RENEWED: &str = include_str!("../tests/fixtures/tls-cert-renewed.pem");
    const CERTIFICATE_OTHER: &str = include_str!("../tests/fixtures/tls-cert-other.pem");
    const KEY_ONLY: &str = include_str!("../tests/fixtures/tls-pubkey.pem");
}
