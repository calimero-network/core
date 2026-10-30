//! Attestation documents: a TDX quote binding the node's transport key (and,
//! when asked, an app's bundle hash), and the check every client and peer runs
//! before sending anything.
//!
//! Report data layout, as today's node uses it:
//! `report_data = nonce(32) || SHA-256("calimero.tee-attest.transport-key.v1" | app_hash | transport_key)`.
//!
//! Asked with `purpose=kms`, a node that took its keys from mero-kms also
//! reports what it verified of that KMS ([`KmsKeys`]) and binds it in:
//! `report_data = nonce(32) || SHA-256("calimero.tee-attest.kms-evidence.v1" |
//! transport_binding | SHA-256(JCS(kms)))`, where `transport_binding` is the
//! second half above. Not asking keeps the layout above, byte for byte.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use calimero_tee_attestation::attest_transport_binding;

/// The mock quote format of `calimero-tee-attestation` (dev only).
pub const MOCK_QUOTE_HEADER: &[u8] = b"MOCK_TDX_QUOTE_V1";
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use url::Url;

pub mod release;

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
    /// The node's receipt-signing key (Ed25519, hex), when the attestation was
    /// asked for it (`purpose=receipts`): then the quote binds this key, not
    /// the transport key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt_key: Option<String>,
    /// The cluster's governance, fixed at genesis, when the attestation was
    /// asked for the receipt key: the quote binds it with the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance: Option<Governance>,
    /// What the node verified of the KMS it took its keys from, when asked
    /// (`purpose=kms`) and it has a record: the quote binds it with the
    /// transport key ([`kms_report_data`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kms: Option<KmsKeys>,
    /// A job node's signing key (Ed25519, hex), when the attestation was asked
    /// for it (`purpose=jobs`): then the quote binds it with the transport key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_key: Option<String>,
    /// The cluster identity (Ed25519, hex) a job node takes work from; the jobs quote binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster_identity: Option<String>,
}

/// What a node verified of the mero-kms replicas it took its keys from, one
/// record per key: the storage key merod fetched itself, and the data disk's
/// key the image's boot script fetched (`merod kms disk-key`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KmsKeys {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<calimero_tee_attestation::KmsEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<calimero_tee_attestation::KmsEvidence>,
}

impl KmsKeys {
    /// No key has a record (a development node, or one whose KMS was pinned by
    /// config rather than a signed release).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.storage.is_none() && self.disk.is_none()
    }

    /// SHA-256 of the records' canonical JSON (RFC 8785): what the quote
    /// binds, and what a verifier in another language recomputes from the
    /// JSON it received.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        use sha2::Digest;
        let canonical = serde_json_canonicalizer::to_vec(self).unwrap_or_default();
        sha2::Sha256::digest(&canonical).into()
    }
}

/// Domain separator for [`kms_evidence_binding`].
pub const ATTEST_KMS_BINDING_DOMAIN: &[u8] = b"calimero.tee-attest.kms-evidence.v1";

/// The second half of a `purpose=kms` quote's report data: SHA-256 over the
/// domain, the transport binding the quote would carry otherwise (`inner`),
/// and [`KmsKeys::digest`]. Wrapping the transport binding keeps the one quote
/// good for both: the key a client seals to, and the KMS the node accepted.
#[must_use]
pub fn kms_evidence_binding(inner: &[u8; 32], kms: &KmsKeys) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(ATTEST_KMS_BINDING_DOMAIN);
    h.update(inner);
    h.update(kms.digest());
    h.finalize().into()
}

/// The report data of a `purpose=kms` attestation.
#[must_use]
pub fn kms_report_data(
    nonce: &[u8; 32],
    app_hash: &[u8; 32],
    transport_key: &[u8; 32],
    kms: &KmsKeys,
) -> [u8; 64] {
    let mut out = [0; 64];
    out[..32].copy_from_slice(nonce);
    out[32..].copy_from_slice(&kms_evidence_binding(
        &attest_transport_binding(app_hash, transport_key),
        kms,
    ));
    out
}

/// Who holds one decision over a cluster's apps. Chosen when the cluster is
/// deployed, recorded in its system state at genesis, and never changed. The
/// same on every cluster, shared or dedicated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Authority {
    /// The parties around each app, with a passkey assertion; the operator
    /// cannot. Which party depends on the decision ([`Governance`]'s fields).
    /// The attested value keeps the name `owner`.
    #[default]
    Owner,
    /// The operator, for every app (the Ops API). Owners cannot. For a
    /// managed deployment whose users trust its operator with this decision.
    Operator,
}

/// The rules a cluster was deployed with, as its replicas attest them: who
/// decides each thing that lets new code, or a new destination, reach an
/// app's data.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Governance {
    /// Which mero-tee releases (node images) may hold the apps' data. `owner`:
    /// each app's **publisher**, the organization its package's signing key
    /// is bound to by the key's own signature (its `APPROVE_RELEASES`
    /// holders), for every instance of the key's packages; for an unbound
    /// package or a raw bundle, the app's own organization or owner account. `operator`: the operator.
    pub releases: Authority,
    /// Who upgrades an app to another bundle. `owner`: per app, as its
    /// package's signed manifest declares (`upgrades`): the instance's owner
    /// or its organization's `MANAGE_APPLICATION` holders (`owner`, the
    /// default), or the publisher's `MANAGE_APPLICATION` holders
    /// (`publisher`; nobody while its key is unbound); a package only ever
    /// loosens toward owners. `operator`:
    /// the operator.
    pub app_upgrades: Authority,
    /// Who approves the URL prefixes an app may call out to: the app's owner
    /// (organization `MANAGE_APPLICATION` holders), per publisher key, or the
    /// operator.
    pub outbound: Authority,
}

impl Governance {
    /// The bytes a quote binds: the JSON, fields in declaration order
    /// (`{"releases":"owner","appUpgrades":"owner","outbound":"owner"}`, what
    /// `JSON.stringify` writes for the same object too).
    #[must_use]
    pub fn canonical(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// What a receipt-key attestation's report data binds after the nonce: the
/// receipt key and the cluster's governance.
#[must_use]
pub fn receipt_key_binding(receipt_key: &[u8; 32], governance: &Governance) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"calimero.tee-attest.receipt-key.v2");
    h.update(receipt_key);
    h.update(sha2::Sha256::digest(governance.canonical()));
    h.finalize().into()
}

/// What a job-node attestation's report data binds after the nonce: both keys, the
/// release, cluster and node id (text u32-BE length-prefixed).
#[must_use]
pub fn job_key_binding(
    transport_key: &[u8; 32],
    job_key: &[u8; 32],
    release: &str,
    cluster: &str,
    node: u64,
    cluster_identity: Option<&[u8; 32]>,
) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"calimero.tee-attest.job-key.v1");
    h.update(transport_key);
    h.update(job_key);
    for text in [release, cluster] {
        h.update((text.len() as u32).to_be_bytes());
        h.update(text.as_bytes());
    }
    h.update(node.to_be_bytes());
    if let Some(identity) = cluster_identity {
        h.update(identity);
    }
    h.finalize().into()
}

/// A node's signed statement that `app`'s entry `index` produced the result
/// whose canonical JSON (RFC 8785) hashes to `result_hash`. The signing key is
/// the node's attested receipt key, so anyone holding the node's attestation
/// can check a receipt later, offline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Receipt {
    pub app: String,
    pub index: u64,
    /// `hex(SHA-256(JCS(result)))`.
    pub result_hash: String,
    pub node: u64,
    /// The receipt key, hex.
    pub key: String,
    /// Ed25519 over [`Receipt::message`], hex.
    pub signature: String,
}

impl Receipt {
    /// The bytes signed.
    #[must_use]
    pub fn message(app: &str, index: u64, result_hash: &str, node: u64) -> Vec<u8> {
        let mut m = b"calimero.receipt.v1\0".to_vec();
        m.extend_from_slice(app.as_bytes());
        m.push(0);
        m.extend_from_slice(&index.to_be_bytes());
        m.extend_from_slice(result_hash.as_bytes());
        m.push(0);
        m.extend_from_slice(&node.to_be_bytes());
        m
    }

    /// The hash a receipt names for `result`.
    #[must_use]
    pub fn hash_result(result: &serde_json::Value) -> String {
        use sha2::Digest;
        let canonical = serde_json_canonicalizer::to_vec(result).unwrap_or_default();
        hex::encode(sha2::Sha256::digest(&canonical))
    }

    /// Whether this receipt is signed by `key` (an attested receipt key) and
    /// names `result`.
    #[must_use]
    pub fn verify(&self, key: &[u8; 32], result: &serde_json::Value) -> bool {
        use ed25519_dalek::Verifier;
        let Ok(vk) = ed25519_dalek::VerifyingKey::from_bytes(key) else {
            return false;
        };
        let Some(sig) = hex::decode(&self.signature)
            .ok()
            .and_then(|b| <[u8; 64]>::try_from(b).ok())
        else {
            return false;
        };
        hex::encode(key) == self.key
            && Self::hash_result(result) == self.result_hash
            && vk
                .verify(
                    &Self::message(&self.app, self.index, &self.result_hash, self.node),
                    &ed25519_dalek::Signature::from_bytes(&sig),
                )
                .is_ok()
    }
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
    /// Trust a release's debug profiles (a debuggable image: the host can read
    /// its memory). Off unless a caller asks; they are never trusted silently.
    pub allow_debug_profiles: bool,
}

/// The oldest mero-tee release a client trusts when it names none: the floor
/// that keeps a node from presenting an old release with known issues.
pub const DEFAULT_MIN_RELEASE: &str = "2.3.109";

/// Whether a release profile is a debug image.
#[must_use]
pub fn is_debug_profile(name: &str) -> bool {
    name.to_ascii_lowercase().contains("debug")
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
    /// `min_version` ([`DEFAULT_MIN_RELEASE`] when none is given).
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
fn claimed_release(named: &str, min_version: Option<&str>) -> eyre::Result<String> {
    use calimero_tee_release::{
        compare_release_versions, normalize_release_version, NODE_RELEASE_TAG_PREFIX,
    };
    let version = normalize_release_version(named, NODE_RELEASE_TAG_PREFIX)
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
///
/// A document that reports the KMS the node verified (`kms`) passes only if
/// its quote binds that record too, so once this returns the record is as
/// trustworthy as the node's own code.
pub async fn verify(
    doc: &AttestationDoc,
    nonce: &[u8; 32],
    policy: &Policy,
) -> eyre::Result<[u8; 32]> {
    let transport_key = hex32(&doc.transport_key)?;
    let app_hash = policy
        .app_hash
        .unwrap_or_else(|| hex32(&doc.app_hash).unwrap_or([0; 32]));
    let expected = match &doc.kms {
        Some(kms) => kms_report_data(nonce, &app_hash, &transport_key, kms),
        None => report_data(nonce, &app_hash, &transport_key),
    };
    verify_report(doc, nonce, policy, &expected).await?;
    Ok(transport_key)
}

/// Verify a receipt-key attestation (`purpose=receipts`): the same checks as
/// [`verify`], over a quote that binds the node's receipt key and the
/// cluster's governance. Answers both.
pub async fn verify_receipt_key(
    doc: &AttestationDoc,
    nonce: &[u8; 32],
    policy: &Policy,
) -> eyre::Result<([u8; 32], Governance)> {
    let key = hex32(
        doc.receipt_key
            .as_deref()
            .ok_or_else(|| eyre::eyre!("the attestation names no receipt key"))?,
    )?;
    let governance = doc
        .governance
        .clone()
        .ok_or_else(|| eyre::eyre!("the attestation names no governance"))?;
    let mut expected = [0; 64];
    expected[..32].copy_from_slice(nonce);
    expected[32..].copy_from_slice(&receipt_key_binding(&key, &governance));
    verify_report(doc, nonce, policy, &expected).await?;
    Ok((key, governance))
}

/// Verify a job-node attestation (`purpose=jobs`): [`verify`]'s checks over a
/// quote binding see [`job_key_binding`]. Answers `(transport_key, job_key)`.
pub async fn verify_job_key(
    doc: &AttestationDoc,
    nonce: &[u8; 32],
    policy: &Policy,
) -> eyre::Result<([u8; 32], [u8; 32])> {
    let transport_key = hex32(&doc.transport_key)?;
    let job_key = hex32(
        doc.job_key
            .as_deref()
            .ok_or_else(|| eyre::eyre!("the attestation names no job key"))?,
    )?;
    let identity = doc.cluster_identity.as_deref().map(hex32).transpose()?;
    let mut expected = [0; 64];
    expected[..32].copy_from_slice(nonce);
    expected[32..].copy_from_slice(&job_key_binding(
        &transport_key,
        &job_key,
        &doc.release.version,
        &doc.cluster,
        doc.node,
        identity.as_ref(),
    ));
    verify_report(doc, nonce, policy, &expected).await?;
    Ok((transport_key, job_key))
}

/// The quote is genuine (or an allowed mock), from a trusted image, and its
/// report data is `expected`.
async fn verify_report(
    doc: &AttestationDoc,
    nonce: &[u8; 32],
    policy: &Policy,
    expected: &[u8; 64],
) -> eyre::Result<()> {
    let _verified = verify_quote(&doc.quote, &doc.release.version, nonce, policy, expected).await?;
    Ok(())
}

/// The measurement registers of a quote, hex, as the quote carries them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuoteMeasurements {
    pub mrtd: String,
    pub rtmr0: String,
    pub rtmr1: String,
    pub rtmr2: String,
    pub rtmr3: String,
}

impl QuoteMeasurements {
    /// The registers of a parsed quote.
    #[must_use]
    pub fn of(body: &calimero_tee_attestation::QuoteBody) -> Self {
        Self {
            mrtd: body.mrtd.clone(),
            rtmr0: body.rtmr0.clone(),
            rtmr1: body.rtmr1.clone(),
            rtmr2: body.rtmr2.clone(),
            rtmr3: body.rtmr3.clone(),
        }
    }
}

/// What a verified real quote showed: its registers, and the release profile
/// they matched (with [`Measurements::Release`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedQuote {
    pub measurements: QuoteMeasurements,
    pub profile: Option<String>,
}

/// The one quote check behind every verifier here: `quote_b64` is genuine (or
/// a mock `policy` allows), comes from an image `policy` trusts (judged
/// against the release the node names, `release`), and its report data is
/// exactly `expected`. Answers the verified registers of a real quote, `None`
/// for a mock.
pub(crate) async fn verify_quote(
    quote_b64: &str,
    release: &str,
    nonce: &[u8; 32],
    policy: &Policy,
    expected: &[u8; 64],
) -> eyre::Result<Option<VerifiedQuote>> {
    let quote = STANDARD.decode(quote_b64)?;
    if quote.starts_with(MOCK_QUOTE_HEADER) {
        if !policy.allow_mock {
            eyre::bail!("mock quote refused: this client only trusts real TEEs");
        }
        let start = MOCK_QUOTE_HEADER.len();
        let report = quote
            .get(start..start + 64)
            .ok_or_else(|| eyre::eyre!("short mock quote"))?;
        if report != expected {
            eyre::bail!("the quote does not bind this key, app and nonce");
        }
        return Ok(None);
    }
    // Refuse what no policy could admit before reaching for the network.
    let release = match &policy.measurements {
        Measurements::None => {
            eyre::bail!(
                "real quote refused: this client trusts no TD image (no measurement policy)"
            )
        }
        Measurements::Pinned(_) => None,
        Measurements::Release { min_version, .. } => Some(claimed_release(
            release,
            Some(min_version.as_deref().unwrap_or(DEFAULT_MIN_RELEASE)),
        )?),
    };
    let mut second = [0; 32];
    second.copy_from_slice(&expected[32..]);
    let result = calimero_tee_attestation::verify_attestation(&quote, nonce, &second)
        .await
        .map_err(|e| eyre::eyre!("quote verification: {e:?}"))?;
    judge(&result, policy, release).await.map(Some)
}

/// A cryptographically checked quote against `policy`'s measurements:
/// `release` is the (floor-checked) release a [`Measurements::Release`]
/// policy reads the allowed registers from.
async fn judge(
    result: &calimero_tee_attestation::VerificationResult,
    policy: &Policy,
    release: Option<String>,
) -> eyre::Result<VerifiedQuote> {
    let b = &result.quote.body;
    let (profile, matched) = match (&policy.measurements, release) {
        (Measurements::Pinned(m), _) => (m.clone(), None),
        (
            Measurements::Release {
                allowed_profiles, ..
            },
            Some(version),
        ) => {
            let release = fetch_release(version.clone())
                .await
                .map_err(|e| eyre::eyre!("mero-tee release {version}: {e:#}"))?;
            let trusted: Vec<String> = allowed_profiles
                .iter()
                .filter(|p| policy.allow_debug_profiles || !is_debug_profile(p))
                .cloned()
                .collect();
            if trusted.is_empty() {
                eyre::bail!(
                    "every allowed profile ({}) is a debug image, which this policy does not trust",
                    allowed_profiles.join(", ")
                );
            }
            let allowed_profiles = &trusted;
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
            (release.profiles[name].clone(), Some(name.to_owned()))
        }
        _ => eyre::bail!("real quote refused: no measurement policy"),
    };
    result
        .policy_valid(&verifier_policy(&profile, &policy.allowed_tcb_statuses))
        .map_err(|r| eyre::eyre!("quote refused: {r:?}"))?;
    Ok(VerifiedQuote {
        measurements: QuoteMeasurements::of(b),
        profile: matched,
    })
}

/// Attests a node's transport key over HTTP, for the sealed client.
#[derive(Debug, Clone)]
pub struct HttpAttestor {
    pub policy: Policy,
    /// Filled with the release the node's verified attestation names, for a
    /// caller that decides by it (a node sending an app's data to a peer).
    pub seen_release: std::sync::Arc<std::sync::Mutex<Option<String>>>,
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
            let key = verify(&doc, &nonce, &self.policy).await?;
            if let Ok(mut seen) = self.seen_release.lock() {
                *seen = Some(doc.release.version.clone());
            }
            Ok(key)
        })
    }
}

fn rand_nonce() -> [u8; 32] {
    ring::rand::generate(&ring::rand::SystemRandom::new())
        .expect("randomness")
        .expose()
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
            receipt_key: None,
            governance: None,
            kms: None,
            job_key: None,
            cluster_identity: None,
        }
    }

    fn mock_job_doc(transport: [u8; 32], job: [u8; 32], nonce: [u8; 32]) -> AttestationDoc {
        let mut report = [0; 64];
        report[..32].copy_from_slice(&nonce);
        report[32..].copy_from_slice(&job_key_binding(
            &transport,
            &job,
            "2.4.0",
            "c",
            1,
            Some(&[4; 32]),
        ));
        let quote = calimero_tee_attestation::generate_mock_attestation(report).quote_bytes;
        AttestationDoc {
            quote: STANDARD.encode(quote),
            transport_key: hex::encode(transport),
            job_key: Some(hex::encode(job)),
            cluster_identity: Some(hex::encode([4; 32])),
            mock: true,
            nonce: hex::encode(nonce),
            ..doc("2.4.0")
        }
    }

    #[tokio::test]
    async fn a_job_key_attestation_binds_both_keys() {
        let mock = Policy {
            allow_mock: true,
            ..Policy::default()
        };
        let nonce = [7; 32];
        let good = mock_job_doc([1; 32], [2; 32], nonce);
        let (t, j) = verify_job_key(&good, &nonce, &mock).await.unwrap();
        assert_eq!((t, j), ([1; 32], [2; 32]));
        let swapped_job = AttestationDoc {
            job_key: Some(hex::encode([3; 32])),
            ..good.clone()
        };
        assert!(verify_job_key(&swapped_job, &nonce, &mock).await.is_err());
        // The identity a job node takes work from is attested: another one does not verify,
        // nor does one left out or put in.
        for identity in [Some(hex::encode([5; 32])), None] {
            let swapped = AttestationDoc {
                cluster_identity: identity,
                ..good.clone()
            };
            assert!(verify_job_key(&swapped, &nonce, &mock).await.is_err());
        }
        let swapped_release = AttestationDoc {
            release: Release {
                version: "2.9.0".into(),
                ..good.release.clone()
            },
            ..good.clone()
        };
        assert!(verify_job_key(&swapped_release, &nonce, &mock)
            .await
            .is_err());
        let swapped_cluster = AttestationDoc {
            cluster: "other".into(),
            ..good.clone()
        };
        assert!(verify_job_key(&swapped_cluster, &nonce, &mock)
            .await
            .is_err());
        let swapped_node = AttestationDoc {
            node: 2,
            ..good.clone()
        };
        assert!(verify_job_key(&swapped_node, &nonce, &mock).await.is_err());
        let swapped_transport = AttestationDoc {
            transport_key: hex::encode([4; 32]),
            ..good.clone()
        };
        assert!(verify_job_key(&swapped_transport, &nonce, &mock)
            .await
            .is_err());
        assert!(verify_job_key(&good, &[8; 32], &mock).await.is_err());
        let none = AttestationDoc {
            job_key: None,
            cluster_identity: None,
            ..good.clone()
        };
        let err = verify_job_key(&none, &nonce, &mock).await.unwrap_err();
        assert!(err.to_string().contains("no job key"), "{err}");
        assert!(verify_job_key(&good, &nonce, &Policy::default())
            .await
            .is_err());
    }

    #[test]
    fn a_document_without_a_job_key_serializes_as_before() {
        let v = serde_json::to_value(doc("2.4.0")).unwrap();
        assert!(v.get("jobKey").is_none());
    }

    /// A receipt holds for its result and key only.
    #[test]
    fn receipts_bind_their_result_and_key() {
        use ed25519_dalek::Signer;
        let sk = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
        let key = sk.verifying_key().to_bytes();
        let result = serde_json::json!({ "b": 1, "a": [1, 2] });
        let hash = Receipt::hash_result(&result);
        let r = Receipt {
            app: "app".into(),
            index: 7,
            result_hash: hash.clone(),
            node: 2,
            key: hex::encode(key),
            signature: hex::encode(sk.sign(&Receipt::message("app", 7, &hash, 2)).to_bytes()),
        };
        assert!(r.verify(&key, &result));
        // Key order does not matter (canonical JSON), the values do.
        assert!(r.verify(&key, &serde_json::json!({ "a": [1, 2], "b": 1 })));
        assert!(!r.verify(&key, &serde_json::json!({ "a": [1, 2], "b": 2 })));
        assert!(!r.verify(&[9; 32], &result));
        let moved = Receipt { index: 8, ..r };
        assert!(!moved.verify(&key, &result));
    }

    #[tokio::test]
    async fn a_real_quote_needs_a_measurement_policy() {
        let err = verify(&doc("2.4.0"), &[2; 32], &Policy::default())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no measurement policy"), "{err}");
    }

    /// Xil's review, issue 14: with no floor named, any old release was trusted.
    #[tokio::test]
    async fn the_default_floor_applies_when_none_is_named() {
        let policy = Policy {
            measurements: Measurements::Release {
                allowed_profiles: vec!["locked-read-only".into()],
                min_version: None,
            },
            ..Policy::default()
        };
        let err = verify(&doc("2.3.9"), &[2; 32], &policy).await.unwrap_err();
        assert!(err.to_string().contains(DEFAULT_MIN_RELEASE), "{err}");
    }

    #[test]
    fn debug_profiles_are_recognised() {
        assert!(is_debug_profile("debug-read-only"));
        assert!(is_debug_profile("locked-DEBUG"));
        assert!(!is_debug_profile("locked-read-only"));
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

    /// A quote that passed the cryptographic check, from a TD whose registers
    /// are all zero (a mock quote's, re-labelled as a real platform's verdict).
    fn checked_quote() -> calimero_tee_attestation::VerificationResult {
        let mut report = [0; 64];
        report[..32].copy_from_slice(&[2; 32]);
        let quote = calimero_tee_attestation::generate_mock_attestation(report).quote_bytes;
        let mut result =
            calimero_tee_attestation::verify_mock_attestation(&quote, &[2; 32], &[0; 32]).unwrap();
        result.tcb_status = Some("UpToDate".into());
        result
    }

    fn pinned(mrtd: &str) -> Policy {
        let zero = "00".repeat(48);
        Policy {
            measurements: Measurements::Pinned(calimero_tee_release::ProfileMeasurements {
                allowed_mrtd: vec![mrtd.to_owned()],
                allowed_rtmr0: vec![],
                allowed_rtmr1: vec![zero.clone()],
                allowed_rtmr2: vec![zero.clone()],
                allowed_rtmr3: vec![zero],
            }),
            ..Policy::default()
        }
    }

    /// The step every real quote passes after DCAP: registers outside the
    /// allowlist are refused, and the verified registers are answered.
    #[tokio::test]
    async fn a_checked_quote_from_another_image_is_refused() {
        let result = checked_quote();
        let ok = judge(&result, &pinned(&"00".repeat(48)), None)
            .await
            .unwrap();
        assert_eq!(ok.measurements.mrtd, "00".repeat(48));
        assert_eq!(ok.profile, None);
        let err = judge(&result, &pinned(&"ab".repeat(48)), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("MeasurementMismatch"), "{err}");
        let err = judge(&result, &Policy::default(), None).await.unwrap_err();
        assert!(err.to_string().contains("no measurement policy"), "{err}");
    }

    fn evidence() -> calimero_tee_attestation::KmsEvidence {
        calimero_tee_attestation::KmsEvidence {
            kms_host: "kms.internal:8080".into(),
            release: "2.4.1".into(),
            profile: Some("locked-read-only".into()),
            mrtd: "aa".repeat(48),
            rtmr0: "00".repeat(48),
            rtmr1: "11".repeat(48),
            rtmr2: "22".repeat(48),
            rtmr3: "33".repeat(48),
            tcb_status: Some("UpToDate".into()),
            quote_sha256: "44".repeat(32),
            verified_at: 1_790_000_000,
            mock: false,
        }
    }

    /// A mock-quoted document over `report`, reporting `kms`.
    fn mock_doc(report: [u8; 64], kms: Option<KmsKeys>) -> AttestationDoc {
        let mut quote = MOCK_QUOTE_HEADER.to_vec();
        quote.extend_from_slice(&report);
        quote.resize(256, 0);
        AttestationDoc {
            quote: STANDARD.encode(quote),
            mock: true,
            kms,
            ..doc("2.4.0")
        }
    }

    fn mock_policy() -> Policy {
        Policy {
            allow_mock: true,
            ..Policy::default()
        }
    }

    /// The KMS a node reports passes only with a quote that binds it: a relay
    /// or host that swaps the record, adds one, or strips it is caught by the
    /// same check that attests the transport key.
    #[tokio::test]
    async fn the_reported_kms_is_bound_into_the_quote() {
        let nonce = [2; 32];
        let transport = [1; 32];
        let kms = KmsKeys {
            storage: Some(evidence()),
            disk: None,
        };
        let bound = kms_report_data(&nonce, &[0; 32], &transport, &kms);
        let key = verify(&mock_doc(bound, Some(kms.clone())), &nonce, &mock_policy())
            .await
            .expect("a quote that binds the record passes");
        assert_eq!(key, transport);

        let mut swapped = kms.clone();
        if let Some(storage) = swapped.storage.as_mut() {
            storage.mrtd = "bb".repeat(48);
        }
        assert!(
            verify(&mock_doc(bound, Some(swapped)), &nonce, &mock_policy())
                .await
                .is_err(),
            "another KMS's registers"
        );
        assert!(
            verify(&mock_doc(bound, None), &nonce, &mock_policy())
                .await
                .is_err(),
            "the record stripped"
        );
        let plain = report_data(&nonce, &[0; 32], &transport);
        assert!(
            verify(&mock_doc(plain, Some(kms)), &nonce, &mock_policy())
                .await
                .is_err(),
            "a record added to a quote that binds none"
        );
        // A node that reports nothing verifies as it always has.
        verify(&mock_doc(plain, None), &nonce, &mock_policy())
            .await
            .expect("an older node's document");
    }

    #[test]
    fn the_kms_binding_never_equals_the_transport_binding() {
        let inner = attest_transport_binding(&[0; 32], &[1; 32]);
        assert_ne!(kms_evidence_binding(&inner, &KmsKeys::default()), inner);
        let one = KmsKeys {
            storage: Some(evidence()),
            disk: None,
        };
        let other = KmsKeys {
            storage: None,
            disk: Some(evidence()),
        };
        assert_ne!(
            kms_evidence_binding(&inner, &one),
            kms_evidence_binding(&inner, &other),
            "which key a record is for is bound too"
        );
    }

    /// Fixed vector, recomputed by the attestation verifier (mero-tee
    /// `attestation-verifier/api/verify.js`) from the JSON a node sends: the
    /// two are separate implementations of one binding, and this keeps them
    /// the same one.
    #[test]
    fn the_kms_binding_matches_the_published_vector() {
        let kms = KmsKeys {
            storage: Some(evidence()),
            disk: Some(calimero_tee_attestation::KmsEvidence {
                profile: None,
                tcb_status: None,
                mock: true,
                ..evidence()
            }),
        };
        assert_eq!(hex::encode(kms.digest()), KMS_EVIDENCE_DIGEST_VECTOR);
        assert_eq!(
            hex::encode(kms_evidence_binding(&[0x11; 32], &kms)),
            KMS_BINDING_VECTOR
        );
    }

    const KMS_EVIDENCE_DIGEST_VECTOR: &str =
        "5e10682b9a71691f98f4a3d2bae3b5e752e7dd217fe16dedfba8396945786666";
    const KMS_BINDING_VECTOR: &str =
        "892223f0ed9f341a78158d181b1edca07a8b8bfeeaadce491361c094abd75aa3";

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
