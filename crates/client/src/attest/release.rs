//! Release attestations: a quote a node makes before it holds any key, so a
//! manager can check which image and which merod a fresh TD runs before mero-kms
//! releases a cluster's key to it.
//!
//! `merod attest` makes one (once, or per request with `--serve`); it needs
//! no KMS key, no store and no running node. Its report data binds the
//! verifier's nonce and what the node claims to be:
//!
//! ```text
//! report_data = nonce(32) || SHA-256("calimero.tee-attest.release.v1"
//!                 || for each of merodVersion, merodSha256, teeRelease, teeProfile, identity:
//!                    u32 BE length || UTF-8 bytes (length 0 when null))
//! ```
//!
//! The second half is what `calimero-tee-verify` takes as `app_hash_hex`, so a
//! verifier that is not Rust checks the quote with that tool and this layout.
//! [`verify_release`] is the Rust check: the same quote check as
//! [`super::verify`] (mock only when allowed, DCAP, the measurement policy, the
//! TCB status), over this report data.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use super::{verify_quote, Policy, QuoteMeasurements};

/// The envelope's format; a verifier refuses any other.
pub const RELEASE_ATTESTATION_FORMAT: u32 = 1;

/// The domain tag of the report data's second half: never equal to a
/// transport, receipt-key or admission quote's.
pub const RELEASE_BINDING_TAG: &[u8] = b"calimero.tee-attest.release.v1";

/// [`ReleaseAttestation::report_data_layout`], for a reader of the JSON.
pub const REPORT_DATA_LAYOUT: &str = "nonce(32) || SHA-256(\"calimero.tee-attest.release.v1\" \
     || for each of merodVersion, merodSha256, teeRelease, teeProfile, identity: \
     u32 BE length || UTF-8 bytes (length 0 when null))";

/// The longest claim field a node may name or a verifier reads.
pub const MAX_CLAIM_FIELD_BYTES: usize = 256;

/// The longest envelope (JSON) [`fetch_release_attestation`] reads: a TDX quote
/// with its certification data is a few KiB.
pub const MAX_RELEASE_ATTESTATION_BYTES: usize = 64 * 1024;

/// How long [`fetch_release_attestation`] waits for an answer.
pub const RELEASE_ATTESTATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the node says it is. Every field is bound into the quote, so none can
/// be changed after it is made; none is proof by itself: the measurements are.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseClaim {
    /// The merod release that made the quote.
    pub merod_version: String,
    /// SHA-256 of the merod executable, hex.
    pub merod_sha256: Option<String>,
    /// The mero-tee release the image says it is (`MERO_TEE_VERSION`): names
    /// the signed measurements a [`super::Measurements::Release`] policy reads.
    pub tee_release: Option<String>,
    /// The image profile (`MERO_TEE_PROFILE`).
    pub tee_profile: Option<String>,
    /// The libp2p peer id of the identity the node will ask mero-kms for a key
    /// under (`--identity`), when it has one before the KMS.
    pub identity: Option<String>,
}

impl ReleaseClaim {
    fn fields(&self) -> [&str; 5] {
        [
            &self.merod_version,
            self.merod_sha256.as_deref().unwrap_or_default(),
            self.tee_release.as_deref().unwrap_or_default(),
            self.tee_profile.as_deref().unwrap_or_default(),
            self.identity.as_deref().unwrap_or_default(),
        ]
    }

    /// Refuse a claim no node makes: a field past [`MAX_CLAIM_FIELD_BYTES`],
    /// an empty version, or an empty string where `null` means "none" (the two
    /// would bind the same bytes).
    pub fn check(&self) -> eyre::Result<()> {
        if self.merod_version.is_empty() {
            eyre::bail!("the claim names no merod version");
        }
        for (name, value) in [
            ("merodSha256", &self.merod_sha256),
            ("teeRelease", &self.tee_release),
            ("teeProfile", &self.tee_profile),
            ("identity", &self.identity),
        ] {
            if value.as_deref() == Some("") {
                eyre::bail!("the claim's {name} is empty; a missing value is null");
            }
        }
        if self
            .fields()
            .iter()
            .any(|f| f.len() > MAX_CLAIM_FIELD_BYTES)
        {
            eyre::bail!("a claim field is longer than {MAX_CLAIM_FIELD_BYTES} bytes");
        }
        Ok(())
    }
}

/// The report data's second half for `claim`.
#[must_use]
pub fn release_binding(claim: &ReleaseClaim) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(RELEASE_BINDING_TAG);
    for field in claim.fields() {
        // SAFETY: `check` bounds every field to MAX_CLAIM_FIELD_BYTES; an
        // unchecked one past u32::MAX cannot exist in memory as a claim field.
        let len = u32::try_from(field.len()).unwrap_or(u32::MAX);
        h.update(len.to_be_bytes());
        h.update(field.as_bytes());
    }
    h.finalize().into()
}

/// The 64 bytes of report data a release attestation's quote carries.
#[must_use]
pub fn release_report_data(nonce: &[u8; 32], claim: &ReleaseClaim) -> [u8; 64] {
    let mut out = [0; 64];
    out[..32].copy_from_slice(nonce);
    out[32..].copy_from_slice(&release_binding(claim));
    out
}

/// What `merod attest` prints and `GET /attestation/release` answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseAttestation {
    /// [`RELEASE_ATTESTATION_FORMAT`].
    pub format: u32,
    /// The quote, base64 (a mock quote from a `--dev` merod).
    pub quote: String,
    /// The verifier's nonce, hex.
    pub nonce: String,
    pub claim: ReleaseClaim,
    /// The quote's 64 bytes of report data, hex ([`release_report_data`]).
    pub report_data: String,
    /// [`REPORT_DATA_LAYOUT`].
    pub report_data_layout: String,
    /// The quote's registers as the node read them; `null` for a mock quote.
    /// Informational: a verifier takes them from the verified quote, and
    /// refuses an envelope whose copy differs.
    pub measurements: Option<QuoteMeasurements>,
    pub mock: bool,
}

impl ReleaseAttestation {
    /// The envelope for a quote over [`release_report_data`]`(nonce, claim)`.
    #[must_use]
    pub fn new(
        quote: &[u8],
        nonce: &[u8; 32],
        claim: ReleaseClaim,
        measurements: Option<QuoteMeasurements>,
        mock: bool,
    ) -> Self {
        let report_data = release_report_data(nonce, &claim);
        Self {
            format: RELEASE_ATTESTATION_FORMAT,
            quote: STANDARD.encode(quote),
            nonce: hex::encode(nonce),
            claim,
            report_data: hex::encode(report_data),
            report_data_layout: REPORT_DATA_LAYOUT.to_owned(),
            measurements,
            mock,
        }
    }
}

/// A release attestation that passed [`verify_release`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedRelease {
    pub claim: ReleaseClaim,
    /// The verified quote's registers; `None` for an allowed mock.
    pub measurements: Option<QuoteMeasurements>,
    /// The release profile they matched (with a `Release` policy).
    pub profile: Option<String>,
}

/// Check `attestation` against the verifier's `nonce` and `policy`: the claim
/// is well formed, the quote binds exactly this nonce and claim, and it passes
/// the same quote check as [`super::verify`] (with a `Release` policy, against
/// the release the claim names). Answers the claim and the verified registers.
pub async fn verify_release(
    attestation: &ReleaseAttestation,
    nonce: &[u8; 32],
    policy: &Policy,
) -> eyre::Result<VerifiedRelease> {
    if attestation.format != RELEASE_ATTESTATION_FORMAT {
        eyre::bail!(
            "release attestation format {} is not {RELEASE_ATTESTATION_FORMAT}",
            attestation.format
        );
    }
    let claim = &attestation.claim;
    claim.check()?;
    if !attestation.nonce.eq_ignore_ascii_case(&hex::encode(nonce)) {
        eyre::bail!("nonce mismatch: the attestation answers another request");
    }
    let expected = release_report_data(nonce, claim);
    if !attestation
        .report_data
        .eq_ignore_ascii_case(&hex::encode(expected))
    {
        eyre::bail!("the report data does not bind this nonce and claim");
    }
    let verified = verify_quote(
        &attestation.quote,
        claim.tee_release.as_deref().unwrap_or_default(),
        nonce,
        policy,
        &expected,
    )
    .await?;
    let Some(verified) = verified else {
        return Ok(VerifiedRelease {
            claim: claim.clone(),
            measurements: None,
            profile: None,
        });
    };
    check_declared(
        attestation,
        &verified.measurements,
        verified.profile.as_deref(),
    )?;
    Ok(VerifiedRelease {
        claim: claim.clone(),
        measurements: Some(verified.measurements),
        profile: verified.profile,
    })
}

/// What the envelope says beside the quote agrees with the verified quote: its
/// copy of the registers, and the profile it claims.
fn check_declared(
    attestation: &ReleaseAttestation,
    measured: &QuoteMeasurements,
    matched_profile: Option<&str>,
) -> eyre::Result<()> {
    if attestation.measurements.as_ref() != Some(measured) {
        eyre::bail!("the envelope's measurements are not the quote's");
    }
    if let (Some(claimed), Some(matched)) =
        (attestation.claim.tee_profile.as_deref(), matched_profile)
    {
        if claimed != matched {
            eyre::bail!("the node claims profile {claimed} but its quote matches {matched}");
        }
    }
    Ok(())
}

/// Ask a node, or a booting TD's `merod attest --serve`, for a release
/// attestation over `nonce`: `GET {base}/attestation/release?nonce=`. Not
/// verified: pass the answer to [`verify_release`].
pub async fn fetch_release_attestation(
    base: &Url,
    nonce: &[u8; 32],
) -> eyre::Result<ReleaseAttestation> {
    let http = reqwest::Client::builder()
        .timeout(RELEASE_ATTESTATION_TIMEOUT)
        .build()?;
    let mut url = calimero_sealed::client::resolve_path(base, "attestation/release")?;
    let _q = url
        .query_pairs_mut()
        .append_pair("nonce", &hex::encode(nonce));
    let response = http.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RELEASE_ATTESTATION_BYTES as u64)
    {
        eyre::bail!("the release attestation is larger than {MAX_RELEASE_ATTESTATION_BYTES} bytes");
    }
    let body = response.bytes().await?;
    if body.len() > MAX_RELEASE_ATTESTATION_BYTES {
        eyre::bail!("the release attestation is larger than {MAX_RELEASE_ATTESTATION_BYTES} bytes");
    }
    Ok(serde_json::from_slice(&body)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attest::{Measurements, MOCK_QUOTE_HEADER};

    fn claim() -> ReleaseClaim {
        ReleaseClaim {
            merod_version: "1.0.0".into(),
            merod_sha256: Some("ab".repeat(32)),
            tee_release: Some("2.4.0".into()),
            tee_profile: Some("locked-read-only".into()),
            identity: None,
        }
    }

    /// A mock quote in the node's format (`calimero_node::attest::quote`).
    fn mock(nonce: &[u8; 32], claim: ReleaseClaim) -> ReleaseAttestation {
        let mut q = MOCK_QUOTE_HEADER.to_vec();
        q.extend_from_slice(&release_report_data(nonce, &claim));
        q.resize(256, 0);
        ReleaseAttestation::new(&q, nonce, claim, None, true)
    }

    fn dev() -> Policy {
        Policy {
            allow_mock: true,
            ..Policy::default()
        }
    }

    /// The layout pinned to bytes, for a verifier written from the docs.
    #[test]
    fn the_binding_is_the_documented_layout() {
        let mut h = Sha256::new();
        h.update(b"calimero.tee-attest.release.v1");
        for f in ["1.0.0", &"ab".repeat(32), "2.4.0", "locked-read-only", ""] {
            h.update(u32::try_from(f.len()).unwrap().to_be_bytes());
            h.update(f.as_bytes());
        }
        let want: [u8; 32] = h.finalize().into();
        assert_eq!(release_binding(&claim()), want);
        assert_eq!(
            hex::encode(release_binding(&ReleaseClaim {
                merod_version: "1.0.0".into(),
                ..ReleaseClaim::default()
            })),
            "68419b869f9a91c2ad517764d903260a3aa20858c870deffa9e692f94166609f"
        );
    }

    #[tokio::test]
    async fn a_mock_release_attestation_verifies_when_mocks_are_allowed() {
        let a = mock(&[7; 32], claim());
        let v = verify_release(&a, &[7; 32], &dev()).await.unwrap();
        assert_eq!(v.claim, claim());
        assert_eq!(v.measurements, None);
        // The JSON round-trips, and the envelope is closed.
        let json = serde_json::to_value(&a).unwrap();
        assert_eq!(
            serde_json::from_value::<ReleaseAttestation>(json.clone()).unwrap(),
            a
        );
        let mut extra = json;
        extra["claim"]["cluster"] = "x".into();
        assert!(serde_json::from_value::<ReleaseAttestation>(extra).is_err());
    }

    #[tokio::test]
    async fn another_nonce_is_refused() {
        let a = mock(&[7; 32], claim());
        let err = verify_release(&a, &[8; 32], &dev()).await.unwrap_err();
        assert!(err.to_string().contains("nonce mismatch"), "{err}");
        // An envelope relabelled with the verifier's nonce still carries a
        // quote over the old one.
        let mut relabelled = a;
        relabelled.nonce = hex::encode([8; 32]);
        relabelled.report_data = hex::encode(release_report_data(&[8; 32], &claim()));
        let err = verify_release(&relabelled, &[8; 32], &dev())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not bind"), "{err}");
    }

    #[tokio::test]
    async fn a_changed_claim_is_refused() {
        let mut a = mock(&[7; 32], claim());
        a.claim.tee_release = Some("2.5.0".into());
        let err = verify_release(&a, &[7; 32], &dev()).await.unwrap_err();
        assert!(err.to_string().contains("report data"), "{err}");
        // Recomputing the envelope's report data does not help: the quote's own differs.
        a.report_data = hex::encode(release_report_data(&[7; 32], &a.claim));
        let err = verify_release(&a, &[7; 32], &dev()).await.unwrap_err();
        assert!(err.to_string().contains("does not bind"), "{err}");
    }

    #[tokio::test]
    async fn malformed_claims_and_formats_are_refused() {
        let mut empty = claim();
        empty.identity = Some(String::new());
        let err = verify_release(&mock(&[7; 32], empty), &[7; 32], &dev())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("null"), "{err}");
        let mut long = claim();
        long.tee_profile = Some("p".repeat(MAX_CLAIM_FIELD_BYTES + 1));
        assert!(verify_release(&mock(&[7; 32], long), &[7; 32], &dev())
            .await
            .is_err());
        let mut v2 = mock(&[7; 32], claim());
        v2.format = 2;
        assert!(verify_release(&v2, &[7; 32], &dev()).await.is_err());
    }

    #[tokio::test]
    async fn a_mock_is_refused_unless_allowed_and_a_real_quote_needs_a_policy() {
        let a = mock(&[7; 32], claim());
        let err = verify_release(&a, &[7; 32], &Policy::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("mock quote refused"), "{err}");

        let mut real = a;
        real.quote = STANDARD.encode(b"not a quote");
        real.mock = false;
        let err = verify_release(&real, &[7; 32], &Policy::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no measurement policy"), "{err}");
        // A Release policy reads the release the claim names, floor first.
        real.claim.tee_release = Some("2.3.9".into());
        real.report_data = hex::encode(release_report_data(&[7; 32], &real.claim));
        let policy = Policy {
            measurements: Measurements::Release {
                allowed_profiles: vec!["locked-read-only".into()],
                min_version: None,
            },
            ..Policy::default()
        };
        let err = verify_release(&real, &[7; 32], &policy).await.unwrap_err();
        assert!(err.to_string().contains("older than the minimum"), "{err}");
    }

    #[test]
    fn declared_measurements_and_profile_must_be_the_quotes() {
        let measured = QuoteMeasurements {
            mrtd: "11".repeat(48),
            rtmr0: "00".repeat(48),
            rtmr1: "22".repeat(48),
            rtmr2: "33".repeat(48),
            rtmr3: "44".repeat(48),
        };
        let mut a = mock(&[7; 32], claim());
        a.measurements = Some(measured.clone());
        assert!(check_declared(&a, &measured, Some("locked-read-only")).is_ok());
        assert!(check_declared(&a, &measured, None).is_ok());
        let err = check_declared(&a, &measured, Some("debug-read-only")).unwrap_err();
        assert!(err.to_string().contains("claims profile"), "{err}");
        let other = QuoteMeasurements {
            rtmr3: "55".repeat(48),
            ..measured.clone()
        };
        let err = check_declared(&a, &other, None).unwrap_err();
        assert!(err.to_string().contains("not the quote's"), "{err}");
        a.measurements = None;
        assert!(check_declared(&a, &measured, None).is_err());
    }
}
