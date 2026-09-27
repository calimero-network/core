//! Talking to a TEE node so that only the attested TD reads the traffic.
//!
//! A node's attestation (`POST /admin-api/tee/attest`) proves what runs inside a
//! TD. It proves nothing about who reads the bytes on the way there unless the
//! quote also commits to a key the client then uses, so both transports here
//! start the same way: ask the node to bind a key into a fresh quote, verify the
//! quote with a [`QuoteVerifier`] the caller trusts, and only then use the key.
//!
//! * [`tls::AttestedTls`] binds the key of the TLS certificate the TD serves and
//!   pins it: a client that accepts only that key knows its TLS connection ends
//!   in the attested TD, whatever certificate authority signed it, or none.
//!   It needs TLS to terminate inside the TD, as it does on mero-tee's image.
//! * [`sealed::SealedTransport`] binds the node's X25519 transport key and
//!   seals every request to it, for a node whose TLS ends outside the TD — at a
//!   proxy, a load balancer, a relay — where TLS alone would expose everything.
//!
//! The two compose: sealed requests can travel over attested TLS.
//!
//! Both are only as good as the verifier. [`PolicyVerifier`] checks the quote
//! against Intel's collateral itself and applies a [`VerifierPolicy`], which
//! must pin the measurements of the image the caller means to trust; a quote
//! that proves only "some genuine TD" proves nothing about which code reads the
//! traffic.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::TeeAttestRequest;
pub use calimero_tee_attestation::VerifierPolicy;
use calimero_tee_attestation::{attest_report_data_suffix, verify_attestation};
use eyre::{bail, eyre, Result, WrapErr};
use serde::Deserialize;
use url::Url;

use crate::connection::{read_body_capped, resolve_path};

pub mod sealed;
pub mod tls;

#[cfg(test)]
mod tests;

/// Where a node attests.
const ATTEST_PATH: &str = "admin-api/tee/attest";
/// Far more than an attestation response ever is: a quote and a few keys.
const MAX_ATTEST_RESPONSE: usize = 256 * 1024;

/// Decides whether a quote is one the caller trusts.
///
/// Implement it to verify quotes some other way — at a verifier service, or
/// against a policy richer than [`VerifierPolicy`] — and hand it to an
/// [`Attestor`]. It must never ask the node being attested whether its own
/// quote is good.
#[async_trait]
pub trait QuoteVerifier: Send + Sync {
    /// Succeed only if `quote` is genuine, comes from a TD running code the
    /// caller accepts, and its report data is `nonce || report_data_suffix`.
    async fn verify(
        &self,
        quote: &[u8],
        nonce: &[u8; 32],
        report_data_suffix: &[u8; 32],
    ) -> Result<()>;
}

/// Verifies a quote here, against Intel's collateral, and applies a policy.
///
/// Collateral comes from Intel PCS unless `CALIMERO_TEE_COLLATERAL_URL` names
/// another source. Mock quotes are never accepted: they carry no signature at
/// all, so accepting one would accept anything.
#[derive(Clone, Debug)]
pub struct PolicyVerifier {
    policy: VerifierPolicy,
}

impl PolicyVerifier {
    /// A verifier enforcing `policy`. Pin `allowed_mrtd` at least: with it
    /// empty every quote is refused.
    #[must_use]
    pub const fn new(policy: VerifierPolicy) -> Self {
        Self { policy }
    }
}

#[async_trait]
impl QuoteVerifier for PolicyVerifier {
    async fn verify(
        &self,
        quote: &[u8],
        nonce: &[u8; 32],
        report_data_suffix: &[u8; 32],
    ) -> Result<()> {
        let verdict = verify_attestation(quote, nonce, report_data_suffix)
            .await
            .map_err(|err| eyre!("the quote did not verify: {err}"))?;
        verdict
            .policy_valid(&self.policy)
            .map_err(|rejection| eyre!("the quote is not one this policy accepts: {rejection:?}"))
    }
}

/// Asks a node to attest and checks the answer with a [`QuoteVerifier`].
#[derive(Clone)]
pub struct Attestor {
    verifier: Arc<dyn QuoteVerifier>,
    application: Option<(ApplicationId, [u8; 32])>,
}

impl std::fmt::Debug for Attestor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attestor")
            .field("application", &self.application)
            .finish_non_exhaustive()
    }
}

/// The keys a verified quote committed to, as [`Attestor::attest`] asked for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AttestedKeys {
    /// The node's X25519 transport key, when asked for.
    pub transport_public_key: Option<[u8; 32]>,
    /// SHA-256 of the served TLS certificate's `SubjectPublicKeyInfo`, when
    /// asked for.
    pub tls_spki_sha256: Option<[u8; 32]>,
}

/// Which keys to have the quote commit to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bind {
    pub transport_key: bool,
    pub tls_key: bool,
}

impl Attestor {
    #[must_use]
    pub fn new(verifier: impl QuoteVerifier + 'static) -> Self {
        Self {
            verifier: Arc::new(verifier),
            application: None,
        }
    }

    /// Also require the node to run `application_id` with exactly this bytecode
    /// hash: the node binds the hash of the application it has installed under
    /// that id, and the quote verifies only if it is this one.
    #[must_use]
    pub const fn with_application(
        mut self,
        application_id: ApplicationId,
        bytecode_hash: [u8; 32],
    ) -> Self {
        self.application = Some((application_id, bytecode_hash));
        self
    }

    /// Ask the node at `api_url` for a quote binding the keys `bind` names,
    /// over `http`, and return them once the quote verifies.
    ///
    /// `http` can be any client, one that would accept an impostor included:
    /// what comes back is trusted because the quote commits to it, not because
    /// of how it arrived. A key an impostor relays from a genuine node is that
    /// node's key, and only that node can use it.
    ///
    /// # Errors
    /// When the node does not answer, predates a binding asked for, or its
    /// quote does not verify.
    pub async fn attest(
        &self,
        http: &reqwest::Client,
        api_url: &Url,
        bind: Bind,
    ) -> Result<AttestedKeys> {
        let mut nonce = [0u8; 32];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut nonce)
            .map_err(|_| eyre!("the system random number generator failed"))?;

        let mut request = TeeAttestRequest::new(
            hex::encode(nonce),
            self.application.as_ref().map(|(id, _)| *id),
        );
        request.bind_transport_key = bind.transport_key;
        request.bind_tls_key = bind.tls_key;

        let response = http
            .post(resolve_path(api_url, ATTEST_PATH)?)
            .json(&request)
            .send()
            .await
            .wrap_err("the node did not answer the attestation request")?;
        let status = response.status();
        let body = read_body_capped(response, MAX_ATTEST_RESPONSE).await?;
        if !status.is_success() {
            bail!(
                "the node refused to attest: HTTP {status}: {}",
                String::from_utf8_lossy(&body)
            );
        }
        let Envelope { data } =
            serde_json::from_slice(&body).wrap_err("the attestation response is malformed")?;

        let keys = AttestedKeys {
            transport_public_key: claimed(
                bind.transport_key,
                data.transport_public_key.as_deref(),
                "transport key",
            )?,
            tls_spki_sha256: claimed(bind.tls_key, data.tls_spki_sha256.as_deref(), "TLS key")?,
        };
        let quote = base64::engine::general_purpose::STANDARD
            .decode(&data.quote_b64)
            .wrap_err("the quote is not base64")?;
        let suffix = report_data_suffix(self.application.as_ref().map(|(_, hash)| hash), &keys);
        self.verifier
            .verify(&quote, &nonce, &suffix)
            .await
            .wrap_err("the attestation did not verify, so the keys it names are not trusted")?;
        Ok(keys)
    }
}

/// What the node puts in report data bytes `32..64` for these keys and this
/// app hash, nested as the attest endpoint nests them.
#[must_use]
pub fn report_data_suffix(app_hash: Option<&[u8; 32]>, keys: &AttestedKeys) -> [u8; 32] {
    attest_report_data_suffix(
        app_hash.copied(),
        keys.tls_spki_sha256.as_ref(),
        keys.transport_public_key.as_ref(),
    )
    .unwrap_or([0; 32])
}

#[derive(Deserialize)]
struct Envelope {
    data: AttestData,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AttestData {
    quote_b64: String,
    #[serde(default)]
    transport_public_key: Option<String>,
    #[serde(default)]
    tls_spki_sha256: Option<String>,
}

/// A 32-byte key the node names in its answer, required when it was asked for.
fn claimed(asked: bool, value: Option<&str>, what: &str) -> Result<Option<[u8; 32]>> {
    if !asked {
        return Ok(None);
    }
    let Some(value) = value else {
        bail!("the node did not name its {what}; it predates binding one into a quote");
    };
    let bytes = hex::decode(value).wrap_err_with(|| format!("the node's {what} is not hex"))?;
    let key = <[u8; 32]>::try_from(bytes.as_slice())
        .map_err(|_| eyre!("the node's {what} is not 32 bytes"))?;
    Ok(Some(key))
}
