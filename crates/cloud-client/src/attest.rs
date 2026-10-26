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
#[derive(Clone, Debug, Default)]
pub struct Policy {
    /// Accept mock quotes (development clusters only).
    pub allow_mock: bool,
    /// The app bundle hash the quote must bind, if the caller cares.
    pub app_hash: Option<[u8; 32]>,
    /// Which TD images a real quote may come from.
    pub measurements: Measurements,
    /// Accepted TCB statuses of a real quote; empty means `UpToDate` only.
    pub allowed_tcb_statuses: Vec<String>,
}

/// How a real quote's measurement registers are judged.
#[derive(Clone, Debug, Default)]
pub enum Measurements {
    /// No image is trusted: every real quote is refused (mock-only clients).
    #[default]
    None,
    /// These allowlists (MRTD and RTMR1-3 required; RTMR0 checked when given).
    Pinned(calimero_tee_release::ProfileMeasurements),
    /// The signed mero-tee release the node names in its attestation: its
    /// Sigstore-verified `published-mrtds.json` must list the quote's registers
    /// under one of `allowed_profiles`, and the release must be at least
    /// `min_version` when one is given.
    Release {
        allowed_profiles: Vec<String>,
        min_version: Option<String>,
    },
}

/// The allowlists a quote is held to once its profile is known.
#[must_use]
pub fn verifier_policy(
    m: &calimero_tee_release::ProfileMeasurements,
    allowed_tcb_statuses: &[String],
) -> calimero_tee_attestation::VerifierPolicy {
    calimero_tee_attestation::VerifierPolicy {
        allowed_tcb_statuses: allowed_tcb_statuses.to_vec(),
        allowed_mrtd: m.allowed_mrtd.clone(),
        allowed_rtmr0: m.allowed_rtmr0.clone(),
        allowed_rtmr1: m.allowed_rtmr1.clone(),
        allowed_rtmr2: m.allowed_rtmr2.clone(),
        allowed_rtmr3: m.allowed_rtmr3.clone(),
        accept_mock: false,
        require_app_hash: true,
    }
}

/// Fetch and Sigstore-verify a release's measurements. The verifier's future is
/// not `Send`, and this runs inside the sealed transport's, so it gets a thread
/// and a runtime of its own.
async fn fetch_release(
    version: String,
) -> eyre::Result<std::sync::Arc<calimero_tee_release::NodeRelease>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _thread = std::thread::Builder::new()
        .name("tee-release-fetch".into())
        .spawn(move || {
            let out = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(eyre::Report::from)
                .and_then(|rt| rt.block_on(calimero_tee_release::fetch_node_release(&version)));
            let _sent = tx.send(out);
        })?;
    rx.await
        .map_err(|_| eyre::eyre!("the release fetch thread stopped"))?
}

/// The release a node names, checked against the floor, before anything is fetched.
fn claimed_release(doc: &AttestationDoc, min_version: Option<&str>) -> eyre::Result<String> {
    use calimero_tee_release::{
        compare_release_versions, normalize_release_version, NODE_RELEASE_TAG_PREFIX,
    };
    let version = normalize_release_version(&doc.release.version, NODE_RELEASE_TAG_PREFIX)
        .map_err(|e| eyre::eyre!("the node names no valid mero-tee release: {e}"))?;
    if let Some(floor) = min_version {
        match compare_release_versions(&version, floor) {
            Some(std::cmp::Ordering::Less) => {
                eyre::bail!("mero-tee release {version} is older than the minimum {floor}")
            }
            Some(_) => {}
            None => eyre::bail!("mero-tee release {version} cannot be compared with {floor}"),
        }
    }
    Ok(version)
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
    // Refuse what no policy could admit before reaching for the network.
    let release = match &policy.measurements {
        Measurements::None => {
            eyre::bail!(
                "real quote refused: this client trusts no TD image (no measurement policy)"
            )
        }
        Measurements::Pinned(_) => None,
        Measurements::Release { min_version, .. } => {
            Some(claimed_release(doc, min_version.as_deref())?)
        }
    };
    let mut second = [0; 32];
    second.copy_from_slice(&expected[32..]);
    let result = calimero_tee_attestation::verify_attestation(&quote, nonce, &second)
        .await
        .map_err(|e| eyre::eyre!("quote verification: {e:?}"))?;
    let b = &result.quote.body;
    let profile = match (&policy.measurements, release) {
        (Measurements::Pinned(m), _) => m.clone(),
        (
            Measurements::Release {
                allowed_profiles, ..
            },
            Some(version),
        ) => {
            let release = fetch_release(version.clone())
                .await
                .map_err(|e| eyre::eyre!("mero-tee release {version}: {e:#}"))?;
            let name = release
                .matching_profile(
                    allowed_profiles,
                    &b.mrtd,
                    &b.rtmr0,
                    &b.rtmr1,
                    &b.rtmr2,
                    &b.rtmr3,
                )
                .ok_or_else(|| {
                    eyre::eyre!(
                        "the quote matches none of the allowed profiles ({}) of release {version}",
                        allowed_profiles.join(", ")
                    )
                })?;
            release.profiles[name].clone()
        }
        _ => eyre::bail!("real quote refused: no measurement policy"),
    };
    result
        .policy_valid(&verifier_policy(&profile, &policy.allowed_tcb_statuses))
        .map_err(|r| eyre::eyre!("quote refused: {r:?}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(version: &str) -> AttestationDoc {
        AttestationDoc {
            // Not a mock quote, so the measurement policy decides first.
            quote: STANDARD.encode(b"not a quote"),
            transport_key: hex::encode([1; 32]),
            app_hash: hex::encode([0; 32]),
            nonce: hex::encode([2; 32]),
            release: Release {
                version: version.into(),
                profile: "locked-read-only".into(),
            },
            cluster: "c".into(),
            node: 1,
            mock: false,
        }
    }

    #[tokio::test]
    async fn a_real_quote_needs_a_measurement_policy() {
        let err = verify(&doc("2.4.0"), &[2; 32], &Policy::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no measurement policy"), "{err}");
    }

    #[tokio::test]
    async fn a_release_below_the_floor_is_refused_before_any_fetch() {
        let policy = Policy {
            measurements: Measurements::Release {
                allowed_profiles: vec!["locked-read-only".into()],
                min_version: Some("2.4.0".into()),
            },
            ..Policy::default()
        };
        let err = verify(&doc("2.3.9"), &[2; 32], &policy).await.unwrap_err();
        assert!(err.to_string().contains("older than the minimum"), "{err}");
        let err = verify(&doc("not-a-version"), &[2; 32], &policy)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("no valid mero-tee release"),
            "{err}"
        );
    }

    #[test]
    fn pinned_measurements_become_a_fail_closed_verifier_policy() {
        let m = calimero_tee_release::ProfileMeasurements {
            allowed_mrtd: vec!["aa".into()],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec!["b1".into()],
            allowed_rtmr2: vec!["b2".into()],
            allowed_rtmr3: vec!["b3".into()],
        };
        let p = verifier_policy(&m, &[]);
        assert_eq!(p.allowed_mrtd, vec!["aa".to_owned()]);
        assert!(p.allowed_tcb_statuses.is_empty()); // UpToDate only
        assert!(!p.accept_mock);
        assert!(p.require_app_hash);
    }
}
