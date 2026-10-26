//! Attestation documents: a TDX quote binding the node's transport key (and,
//! when asked, an app's bundle hash), and the check every client and peer runs
//! before sending anything.
//!
//! Report data layout, as today's node uses it:
//! `report_data = nonce(32) || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_tee_attestation::attest_transport_binding;

/// The mock quote format of `calimero-tee-attestation` (dev only).
pub const MOCK_QUOTE_HEADER: &[u8] = b"MOCK_TDX_QUOTE_V1";
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub profile: String,
}

/// `GET /attestation` answers this.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttestationDoc {
    /// The quote, base64.
    pub quote: String,
    /// X25519 transport key, hex.
    pub transport_key: String,
    /// The app's bundle hash bound into the quote, hex (zeros when none).
    pub app_hash: String,
    pub nonce: String,
    pub release: Release,
    pub cluster: String,
    pub node: u64,
    pub mock: bool,
}

/// Build the 64 bytes of report data.
#[must_use]
pub fn report_data(nonce: &[u8; 32], app_hash: &[u8; 32], transport_key: &[u8; 32]) -> [u8; 64] {
    let mut out = [0; 64];
    out[..32].copy_from_slice(nonce);
    out[32..].copy_from_slice(&attest_transport_binding(app_hash, transport_key));
    out
}

/// What a verifier accepts.
#[derive(Clone, Debug)]
pub struct Policy {
    /// Accept mock quotes (development clusters only).
    pub allow_mock: bool,
    /// The app bundle hash the quote must bind, if the caller cares.
    pub app_hash: Option<[u8; 32]>,
}

fn hex32(s: &str) -> eyre::Result<[u8; 32]> {
    let v = hex::decode(s)?;
    v.try_into().map_err(|_| eyre::eyre!("expected 32 bytes"))
}

/// Check `doc` against `nonce` and `policy`; returns the attested transport key.
pub async fn verify(
    doc: &AttestationDoc,
    nonce: &[u8; 32],
    policy: &Policy,
) -> eyre::Result<[u8; 32]> {
    let quote = STANDARD.decode(&doc.quote)?;
    let transport_key = hex32(&doc.transport_key)?;
    let app_hash = policy
        .app_hash
        .unwrap_or_else(|| hex32(&doc.app_hash).unwrap_or([0; 32]));
    let expected = report_data(nonce, &app_hash, &transport_key);
    if quote.starts_with(MOCK_QUOTE_HEADER) {
        if !policy.allow_mock {
            eyre::bail!("mock quote refused: this client only trusts real TEEs");
        }
        let start = MOCK_QUOTE_HEADER.len();
        let report = quote
            .get(start..start + 64)
            .ok_or_else(|| eyre::eyre!("short mock quote"))?;
        if report != expected {
            eyre::bail!("the quote does not bind this transport key, app and nonce");
        }
        return Ok(transport_key);
    }
    let mut second = [0; 32];
    second.copy_from_slice(&expected[32..]);
    let result = calimero_tee_attestation::verify_attestation(&quote, nonce, &second)
        .await
        .map_err(|e| eyre::eyre!("quote verification: {e:?}"))?;
    if !result.is_valid() {
        eyre::bail!("quote verification failed");
    }
    // TODO(release-v2): check the measurements against the Sigstore-verified
    // published-mrtds.json via calimero-tee-release.
    Ok(transport_key)
}

/// Attests a node's transport key over HTTP, for the sealed client.
#[derive(Debug, Clone)]
pub struct HttpAttestor {
    pub policy: Policy,
}

impl calimero_sealed::client::Attest for HttpAttestor {
    fn attest_transport_key<'a>(
        &'a self,
        http: &'a reqwest::Client,
        api_url: &'a Url,
    ) -> BoxFuture<'a, eyre::Result<[u8; 32]>> {
        Box::pin(async move {
            let nonce: [u8; 32] = rand_nonce();
            let mut url = calimero_sealed::client::resolve_path(api_url, "attestation")?;
            let _q = url
                .query_pairs_mut()
                .append_pair("nonce", &hex::encode(nonce));
            if let Some(h) = self.policy.app_hash {
                let _q = url
                    .query_pairs_mut()
                    .append_pair("appHash", &hex::encode(h));
            }
            let doc: AttestationDoc = http
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            verify(&doc, &nonce, &self.policy).await
        })
    }
}

fn rand_nonce() -> [u8; 32] {
    let mut n = [0; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut n).expect("randomness");
    n
}
