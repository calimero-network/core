//! TLS pinned to the key an attested TD serves.
//!
//! A certificate authority vouches for who controls a domain, not for where a
//! key lives: an operator, a proxy or anyone else who controls the domain can
//! obtain a certificate the usual trust store accepts, and read everything that
//! crosses it. On a node whose TLS terminates inside the TD (mero-tee's image
//! terminates it in the TD, with a key generated there), the quote can commit
//! to the certificate's key instead. [`AttestedTls::connect`] checks that
//! commitment, then hands back a client that completes a TLS handshake only
//! with the holder of exactly that key — so the far end of every connection it
//! makes is the attested TD.
//!
//! Certificate authorities, names and expiry play no part: the pin is the whole
//! check, and the handshake signature still proves the server holds the key.
//! That also makes a node reachable by its bare IP address, or before it has
//! a publicly trusted certificate, with nothing weaker than a CA would give.

use std::sync::Arc;

use calimero_tee_attestation::tls_spki_sha256;
use eyre::{bail, eyre, Result, WrapErr};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use url::Url;

use super::{Attestor, Bind};

/// A client whose TLS connections end in the attested TD, and nowhere else.
#[derive(Clone, Debug)]
pub struct AttestedTls {
    client: reqwest::Client,
    spki_sha256: [u8; 32],
}

impl AttestedTls {
    /// Attest the node at `api_url` (an `https` URL), binding the key of the
    /// TLS certificate it serves, and pin that key.
    ///
    /// The attestation request itself goes over a connection that accepts any
    /// key: nothing in its answer is trusted until the quote verifies, and the
    /// key the quote commits to is then the only one accepted. An impostor that
    /// relays the request to a genuine node gets that node's key pinned, which
    /// only that node can use.
    ///
    /// # Errors
    /// When `api_url` is not `https`, the node does not bind a TLS key (it
    /// names no attested certificate, or predates the binding), or its quote
    /// does not verify.
    pub async fn connect(api_url: &Url, attestor: &Attestor) -> Result<Self> {
        if api_url.scheme() != "https" {
            bail!("attested TLS needs an https URL, got {api_url}");
        }
        let probe = client_with(PinnedKey::any())?;
        let keys = attestor
            .attest(
                &probe,
                api_url,
                Bind {
                    tls_key: true,
                    ..Bind::default()
                },
            )
            .await?;
        let spki_sha256 = keys
            .tls_spki_sha256
            .ok_or_else(|| eyre!("the attestation named no TLS key"))?;
        Self::pinned(spki_sha256)
    }

    /// A client pinned to a key verified some other way: `spki_sha256` is the
    /// SHA-256 of the certificate's `SubjectPublicKeyInfo`.
    ///
    /// # Errors
    /// When the TLS stack cannot be set up.
    pub fn pinned(spki_sha256: [u8; 32]) -> Result<Self> {
        Ok(Self {
            client: client_with(PinnedKey::to(spki_sha256))?,
            spki_sha256,
        })
    }

    /// The client: every connection it makes is pinned.
    #[must_use]
    pub const fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// SHA-256 of the pinned key's `SubjectPublicKeyInfo`.
    #[must_use]
    pub const fn spki_sha256(&self) -> &[u8; 32] {
        &self.spki_sha256
    }
}

fn client_with(verifier: PinnedKey) -> Result<reqwest::Client> {
    let provider = Arc::clone(&verifier.provider);
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .wrap_err("the TLS stack has no protocol versions")?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    reqwest::Client::builder()
        .use_preconfigured_tls(config)
        // A redirect could hand the request to a URL the caller never named.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .wrap_err("failed to build the pinned HTTP client")
}

/// Accepts a server whose certificate carries the pinned key (or any key,
/// before one is pinned) and proves it holds it.
#[derive(Debug)]
struct PinnedKey {
    /// `None` accepts any key: only for the attestation request, whose answer
    /// is trusted for what the quote commits to, not for how it arrived.
    spki_sha256: Option<[u8; 32]>,
    provider: Arc<CryptoProvider>,
}

impl PinnedKey {
    fn any() -> Self {
        Self {
            spki_sha256: None,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
        }
    }

    fn to(spki_sha256: [u8; 32]) -> Self {
        Self {
            spki_sha256: Some(spki_sha256),
            ..Self::any()
        }
    }
}

impl ServerCertVerifier for PinnedKey {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let Some(pin) = &self.spki_sha256 else {
            return Ok(ServerCertVerified::assertion());
        };
        let presented = tls_spki_sha256(end_entity).map_err(|_| {
            rustls::Error::InvalidCertificate(rustls::CertificateError::BadEncoding)
        })?;
        if presented != *pin {
            return Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ));
        }
        Ok(ServerCertVerified::assertion())
    }

    // The signatures are what prove the server holds the key the certificate
    // names; without them a certificate is public data anybody can present.
    // They are checked the same way whether or not a key is pinned.
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
