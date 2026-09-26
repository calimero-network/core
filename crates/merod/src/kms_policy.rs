//! KMS attestation policy for merod.
//!
//! When MERO_KMS_RELEASE_TAG, MERO_KMS_VERSION, or MERO_TEE_VERSION is set,
//! merod fetches the attestation policy from the official mero-tee release
//! instead of relying on config written by external scripts.
//! Use USE_ENV_POLICY=true for air-gapped deployments
//! (requires policy in config.toml via apply-merod-kms-phala-attestation-config.sh).
//!
//! With MERO_TEE_PROFILE set, merod asks for that profile's policy asset
//! (`kms-phala-attestation-policy.<profile>.json`) and falls back to the
//! generic asset only when the release does not publish one. Either way the
//! signed file's `profile` must equal MERO_TEE_PROFILE, so the fallback can
//! never verify one profile's KMS against another profile's measurements.

use base64::Engine;
use eyre::{bail, Result as EyreResult};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sigstore::bundle::verify::policy::{
    AllOf, GitHubWorkflowName, GitHubWorkflowRef, GitHubWorkflowRepository, GitHubWorkflowTrigger,
    OIDCIssuer, SingleX509ExtPolicy, VerificationPolicy as SigstoreVerificationPolicy,
};
use sigstore::bundle::verify::Verifier as SigstoreBundleVerifier;
use sigstore::cosign::bundle::SignedArtifactBundle;
use sigstore::crypto::{CosignVerificationKey, Signature as SigstoreSignature, SigningScheme};
use sigstore::trust::sigstore::SigstoreTrustRoot;
use sigstore::trust::TrustRoot;
use tracing::{info, warn};
use x509_cert::der::{DecodePem, Encode};
use x509_cert::Certificate;

const POLICY_RELEASE_BASE: &str = "https://github.com/calimero-network/mero-tee/releases/download";
const POLICY_FETCH_RETRIES: usize = 3;
/// Upper bound on the exponential fetch backoff. Without it a large `attempt`
/// would saturate to `u64::MAX` milliseconds (~585 million years); this caps
/// the wait at a practical ceiling.
const POLICY_FETCH_MAX_BACKOFF_MS: u64 = 60_000;
const DEFAULT_ALLOWED_TCB_STATUSES: &[&str] = &["uptodate"];
/// Release assets are `<stem>.json` (generic) and `<stem>.<profile>.json`
/// (per image profile), each with a `.sig` and a `.bundle.json` beside it.
const POLICY_ASSET_STEM: &str = "kms-phala-attestation-policy";
const SIGSTORE_OIDC_ISSUER: &str = "https://token.actions.githubusercontent.com";
const SIGSTORE_WORKFLOW_TRIGGER: &str = "push";
const SIGSTORE_WORKFLOW_NAME: &str = "Release mero-kms";
const SIGSTORE_WORKFLOW_REPOSITORY: &str = "calimero-network/mero-tee";
const SIGSTORE_WORKFLOW_REF: &str = "refs/heads/master";

/// Attestation policy for KMS verification (mirrors mero-kms AttestationPolicy).
#[derive(Debug, Clone)]
pub struct KmsAttestationPolicy {
    /// Allowed TCB statuses (normalized to lowercase).
    pub allowed_tcb_statuses: Vec<String>,
    /// Allowed MRTD values (hex, lowercase, no 0x prefix).
    pub allowed_mrtd: Vec<String>,
    /// Allowed RTMR0-3 values (hex, lowercase, no 0x prefix).
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    /// Default binding for KMS /attest (base64).
    pub default_binding_b64: String,
}

/// Root structure of the release policy JSON.
#[derive(Debug, Deserialize)]
struct PolicyJson {
    /// What the file describes. A published release policy says `"kms"`; any
    /// other value is a file for something else and must not be read as one.
    #[serde(default)]
    role: Option<String>,
    /// The image profile whose KMS the file pins (`locked-read-only`, ...).
    #[serde(default)]
    profile: Option<String>,
    /// The release the file belongs to, without the `mero-kms-v` prefix.
    #[serde(default)]
    tag: Option<String>,
    #[serde(default)]
    policy: PolicySection,
    #[serde(default)]
    kms: KmsSection,
}

/// The allowlists the KMS's own quote is checked against.
///
/// Published policies name these `kms_allowed_*` and carry the NODE allowlists
/// beside them as `node_allowed_*`. Those describe the nodes the KMS serves, not
/// the KMS, so they are deliberately not read here: a KMS whose measurements
/// happened to match a node image would otherwise pass. The unprefixed
/// `allowed_*` names are an older layout, read only when the prefixed ones are
/// absent.
#[derive(Debug, Deserialize, Default)]
struct PolicySection {
    #[serde(default)]
    kms_allowed_tcb_statuses: Vec<String>,
    #[serde(default)]
    kms_allowed_mrtd: Vec<String>,
    #[serde(default)]
    kms_allowed_rtmr0: Vec<String>,
    #[serde(default)]
    kms_allowed_rtmr1: Vec<String>,
    #[serde(default)]
    kms_allowed_rtmr2: Vec<String>,
    #[serde(default)]
    kms_allowed_rtmr3: Vec<String>,
    #[serde(default)]
    allowed_tcb_statuses: Vec<String>,
    #[serde(default)]
    allowed_mrtd: Vec<String>,
    #[serde(default)]
    allowed_rtmr0: Vec<String>,
    #[serde(default)]
    allowed_rtmr1: Vec<String>,
    #[serde(default)]
    allowed_rtmr2: Vec<String>,
    #[serde(default)]
    allowed_rtmr3: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
struct KmsSection {
    #[serde(default)]
    default_binding_b64: String,
}

/// Read release version from environment with explicit precedence:
/// `MERO_KMS_RELEASE_TAG` > `MERO_KMS_VERSION` > `MERO_TEE_VERSION`.
///
/// Values may be either a plain version (`2.1.14`) or a prefixed tag
/// (`mero-kms-v2.1.14`). Invalid values return an error.
pub fn release_version_from_env() -> EyreResult<Option<String>> {
    release_version_from_env_with(|env_var| std::env::var(env_var).ok())
}

fn release_version_from_env_with<F>(mut env_reader: F) -> EyreResult<Option<String>>
where
    F: FnMut(&str) -> Option<String>,
{
    for env_var in [
        "MERO_KMS_RELEASE_TAG",
        "MERO_KMS_VERSION",
        "MERO_TEE_VERSION",
    ] {
        if let Some(raw) = env_reader(env_var) {
            if raw.trim().is_empty() {
                // Empty env vars are treated as unset so lower-priority values can apply.
                continue;
            }
            return normalize_release_version(&raw)
                .map(Some)
                .map_err(|e| eyre::eyre!("{env_var} is invalid: {e}"));
        }
    }

    Ok(None)
}

/// Whether to skip release fetch and use config.toml policy (air-gapped).
pub fn use_env_policy() -> bool {
    // Security note: this bypasses release-policy fetch. It is intended for
    // controlled environments (for example air-gapped deployments) where policy
    // files are provisioned and verified by deployment tooling.
    std::env::var("USE_ENV_POLICY")
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Fetch attestation policy from the official mero-tee release.
///
/// Trust model:
/// - fetches policy, detached signature, and Sigstore bundle from release assets
/// - verifies Rekor signed entry timestamp and detached signature over policy bytes
/// - verifies Fulcio certificate chain and GitHub workflow identity constraints
///
/// Which policy is fetched follows `MERO_TEE_PROFILE`; see [`policy_asset_candidates`].
pub async fn fetch_policy_from_release(version: &str) -> EyreResult<KmsAttestationPolicy> {
    let version = normalize_release_version(version)?;
    let tag = format!("mero-kms-v{version}");
    let expected_profile = expected_profile_from_env();
    let candidates = policy_asset_candidates(expected_profile.as_deref())?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("merod/1.0")
        .build()
        .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;

    let (assets, bodies) =
        fetch_first_published_policy(&client, POLICY_RELEASE_BASE, &tag, &candidates).await?;

    verify_policy_signature(&bodies, assets)
        .await
        .map_err(|e| eyre::eyre!("Policy signature verification failed: {}", e))?;

    info!(asset = %assets.json, "Verified KMS attestation policy signature");
    parse_policy_json_for_release(&bodies.policy, &version, expected_profile.as_deref())
}

/// The three release assets that together make one signed policy.
#[derive(Debug, PartialEq, Eq)]
struct PolicyAssets {
    json: String,
    signature: String,
    bundle: String,
}

impl PolicyAssets {
    fn for_json(json: String) -> Self {
        Self {
            signature: format!("{json}.sig"),
            bundle: format!("{json}.bundle.json"),
            json,
        }
    }
}

/// The downloaded contents of a [`PolicyAssets`].
struct PolicyBodies {
    policy: String,
    signature: String,
    bundle: String,
}

/// The policy assets to try, in order.
///
/// A node with a profile asks for that profile's policy first. The generic
/// asset follows it only as a fallback for releases that publish no per-profile
/// file: it is the locked-read-only policy, so on any other profile its
/// `profile` field fails the check in [`parse_policy_json_for_release`] rather
/// than admitting a KMS with the wrong measurements.
fn policy_asset_candidates(profile: Option<&str>) -> EyreResult<Vec<PolicyAssets>> {
    let generic = PolicyAssets::for_json(format!("{POLICY_ASSET_STEM}.json"));
    let Some(profile) = profile else {
        return Ok(vec![generic]);
    };
    // The profile becomes part of a URL path, so it is held to the shape
    // profile names have rather than trusted to be one.
    if !profile
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        || profile.starts_with('-')
    {
        bail!(
            "MERO_TEE_PROFILE is invalid: expected lowercase letters, digits and '-', got {profile:?}"
        );
    }
    let per_profile = PolicyAssets::for_json(format!("{POLICY_ASSET_STEM}.{profile}.json"));
    Ok(vec![per_profile, generic])
}

/// Fetch the first candidate whose policy JSON the release publishes.
///
/// Only a 404 on a policy JSON moves on to the next candidate. A published
/// policy whose signature or bundle is missing is an error, not a reason to
/// try another file.
async fn fetch_first_published_policy<'a>(
    client: &reqwest::Client,
    base: &str,
    tag: &str,
    candidates: &'a [PolicyAssets],
) -> EyreResult<(&'a PolicyAssets, PolicyBodies)> {
    for (index, assets) in candidates.iter().enumerate() {
        let url = |asset: &str| format!("{base}/{tag}/{asset}");
        let Some(policy) =
            fetch_release_asset_with_retry(client, &url(&assets.json), &assets.json).await?
        else {
            warn!(
                release = tag,
                asset = %assets.json,
                "Release publishes no such policy asset"
            );
            continue;
        };
        if index > 0 {
            warn!(
                release = tag,
                asset = %assets.json,
                "Falling back to the generic policy; its profile must still match MERO_TEE_PROFILE"
            );
        }
        let signature =
            fetch_release_asset_with_retry(client, &url(&assets.signature), &assets.signature)
                .await?
                .ok_or_else(|| {
                    eyre::eyre!(
                        "Release {tag} publishes {} but not {}",
                        assets.json,
                        assets.signature
                    )
                })?;
        let bundle = fetch_release_asset_with_retry(client, &url(&assets.bundle), &assets.bundle)
            .await?
            .ok_or_else(|| {
                eyre::eyre!(
                    "Release {tag} publishes {} but not {}",
                    assets.json,
                    assets.bundle
                )
            })?;
        return Ok((
            assets,
            PolicyBodies {
                policy,
                signature,
                bundle,
            },
        ));
    }

    let names: Vec<&str> = candidates.iter().map(|a| a.json.as_str()).collect();
    bail!(
        "Release {tag} publishes none of the policy assets {}",
        names.join(", ")
    )
}

/// Fetch one release asset, retrying transient failures. `Ok(None)` means the
/// release has no such asset (404).
async fn fetch_release_asset_with_retry(
    client: &reqwest::Client,
    url: &str,
    asset_name: &str,
) -> EyreResult<Option<String>> {
    let mut attempt = 1;
    loop {
        match fetch_release_asset(client, url, asset_name).await {
            AssetFetchResult::Success(body) => return Ok(Some(body)),
            AssetFetchResult::NotFound => return Ok(None),
            AssetFetchResult::Permanent(error) => bail!("{error}"),
            AssetFetchResult::Transient(error) if attempt < POLICY_FETCH_RETRIES => {
                warn!(
                    attempt,
                    retries = POLICY_FETCH_RETRIES,
                    asset = asset_name,
                    error = %error,
                    "Transient policy asset fetch status, retrying"
                );
                tokio::time::sleep(policy_fetch_backoff(attempt)).await;
                attempt += 1;
            }
            AssetFetchResult::Transient(error) => bail!("{error}"),
        }
    }
}

enum AssetFetchResult {
    Success(String),
    NotFound,
    Transient(String),
    Permanent(String),
}

async fn fetch_release_asset(
    client: &reqwest::Client,
    url: &str,
    asset_name: &str,
) -> AssetFetchResult {
    match client.get(url).send().await {
        Ok(resp) if resp.status().is_success() => match resp.text().await {
            Ok(body) => AssetFetchResult::Success(body),
            Err(err) => AssetFetchResult::Transient(format!(
                "Failed to read {asset_name} response body: {err}"
            )),
        },
        Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => AssetFetchResult::NotFound,
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            let error = format!("Failed to fetch {asset_name}: {status} {url} {body}");
            if status.is_server_error() || status.as_u16() == 429 {
                AssetFetchResult::Transient(error)
            } else {
                AssetFetchResult::Permanent(error)
            }
        }
        Err(err) => AssetFetchResult::Transient(format!("Failed to fetch {asset_name}: {err}")),
    }
}

async fn verify_policy_signature(bodies: &PolicyBodies, assets: &PolicyAssets) -> EyreResult<()> {
    let trust_root = SigstoreTrustRoot::new(None)
        .await
        .map_err(|e| eyre::eyre!("Failed to initialize Sigstore trust root: {}", e))?;
    let rekor_pub_keys = rekor_public_keys(&trust_root)?;
    let signed_bundle = SignedArtifactBundle::new_verified(&bodies.bundle, &rekor_pub_keys)
        .map_err(|e| eyre::eyre!("Invalid signed bundle: {}", e))?;

    let detached_signature = bodies.signature.trim();
    if detached_signature.is_empty() {
        bail!("Policy signature asset is empty");
    }
    if detached_signature != signed_bundle.base64_signature.trim() {
        bail!(
            "Policy signature mismatch between {} and {}",
            assets.signature,
            assets.bundle
        );
    }
    let policy_body = bodies.policy.as_str();

    let certificate_pem = decode_bundle_certificate_pem(&signed_bundle.cert)?;
    verify_blob_signature(
        policy_body.as_bytes(),
        &signed_bundle.base64_signature,
        &certificate_pem,
    )?;

    let policy_bundle =
        build_policy_sigstore_bundle(policy_body.as_bytes(), &signed_bundle, &certificate_pem)?;
    let oidc_issuer = OIDCIssuer::new(SIGSTORE_OIDC_ISSUER);
    let workflow_trigger = GitHubWorkflowTrigger::new(SIGSTORE_WORKFLOW_TRIGGER);
    let workflow_name = GitHubWorkflowName::new(SIGSTORE_WORKFLOW_NAME);
    let workflow_repository = GitHubWorkflowRepository::new(SIGSTORE_WORKFLOW_REPOSITORY);
    let workflow_ref = GitHubWorkflowRef::new(SIGSTORE_WORKFLOW_REF);

    let workflow_policy = AllOf::new([
        &oidc_issuer as &dyn SigstoreVerificationPolicy,
        &workflow_trigger,
        &workflow_name,
        &workflow_repository,
        &workflow_ref,
    ])
    .ok_or_else(|| eyre::eyre!("Failed to construct Sigstore verification policy"))?;

    let verifier = SigstoreBundleVerifier::new(Default::default(), trust_root)
        .map_err(|e| eyre::eyre!("Failed to create Sigstore verifier: {}", e))?;
    verifier
        .verify(
            policy_body.as_bytes(),
            policy_bundle,
            &workflow_policy,
            true,
        )
        .await
        .map_err(|e| eyre::eyre!("Sigstore bundle verification failed: {}", e))?;

    Ok(())
}

fn rekor_public_keys(
    trust_root: &SigstoreTrustRoot,
) -> EyreResult<std::collections::BTreeMap<String, CosignVerificationKey>> {
    let mut keys = std::collections::BTreeMap::new();
    for (key_id, key_der) in trust_root
        .rekor_keys()
        .map_err(|e| eyre::eyre!("Failed to read Rekor keys from trust root: {}", e))?
    {
        match CosignVerificationKey::from_der(key_der, &SigningScheme::default()) {
            Ok(key) => {
                keys.insert(key_id, key);
            }
            Err(err) => {
                warn!(
                    rekor_key_id = %key_id,
                    error = %err,
                    "Skipping unsupported Rekor key from Sigstore trust root"
                );
            }
        }
    }

    if keys.is_empty() {
        bail!("Sigstore trust root did not provide a usable Rekor public key");
    }

    Ok(keys)
}

fn decode_bundle_certificate_pem(encoded_cert: &str) -> EyreResult<String> {
    let cert_bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded_cert.trim())
        .map_err(|e| eyre::eyre!("Bundle certificate is not valid base64: {}", e))?;
    String::from_utf8(cert_bytes)
        .map_err(|e| eyre::eyre!("Bundle certificate is not valid UTF-8 PEM: {}", e))
}

fn verify_blob_signature(
    policy_body: &[u8],
    signature_b64: &str,
    cert_pem: &str,
) -> EyreResult<()> {
    let certificate = Certificate::from_pem(cert_pem.as_bytes())
        .map_err(|e| eyre::eyre!("Failed to parse policy signing certificate PEM: {}", e))?;
    let spki_der = certificate
        .tbs_certificate()
        .subject_public_key_info()
        .to_der()?;
    let verification_key = CosignVerificationKey::try_from_der(&spki_der).map_err(|e| {
        eyre::eyre!(
            "Failed to extract verification key from signing certificate: {}",
            e
        )
    })?;
    verification_key
        .verify_signature(
            SigstoreSignature::Base64Encoded(signature_b64.trim().as_bytes()),
            policy_body,
        )
        .map_err(|e| eyre::eyre!("Detached signature does not match policy body: {}", e))
}

fn build_policy_sigstore_bundle(
    policy_body: &[u8],
    signed_bundle: &SignedArtifactBundle,
    certificate_pem: &str,
) -> EyreResult<sigstore::bundle::Bundle> {
    let signature_bytes = base64::engine::general_purpose::STANDARD
        .decode(signed_bundle.base64_signature.trim())
        .map_err(|e| eyre::eyre!("Bundle signature is not valid base64: {}", e))?;
    let signed_entry_timestamp = base64::engine::general_purpose::STANDARD
        .decode(signed_bundle.rekor_bundle.signed_entry_timestamp.trim())
        .map_err(|e| eyre::eyre!("Bundle signed entry timestamp is not valid base64: {}", e))?;
    let canonicalized_body = base64::engine::general_purpose::STANDARD
        .decode(signed_bundle.rekor_bundle.payload.body.trim())
        .map_err(|e| eyre::eyre!("Bundle canonicalized body is not valid base64: {}", e))?;
    let canonicalized_body_json: serde_json::Value = serde_json::from_slice(&canonicalized_body)
        .map_err(|e| eyre::eyre!("Bundle canonicalized body is not valid JSON: {}", e))?;
    let kind = canonicalized_body_json
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| eyre::eyre!("Bundle canonicalized body is missing kind"))?;
    let api_version = canonicalized_body_json
        .get("apiVersion")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| eyre::eyre!("Bundle canonicalized body is missing apiVersion"))?;
    let log_id = hex::decode(signed_bundle.rekor_bundle.payload.log_id.trim())
        .map_err(|e| eyre::eyre!("Bundle log ID is not valid hex: {}", e))?;

    let certificate = Certificate::from_pem(certificate_pem.as_bytes())
        .map_err(|e| eyre::eyre!("Failed to parse bundle certificate PEM: {}", e))?;
    let cert_der = certificate
        .to_der()
        .map_err(|e| eyre::eyre!("Failed to encode bundle certificate DER: {}", e))?;
    let digest = Sha256::digest(policy_body);

    let bundle_json = serde_json::json!({
        "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.1",
        "verificationMaterial": {
            "x509CertificateChain": {
                "certificates": [{
                    "rawBytes": base64::engine::general_purpose::STANDARD.encode(cert_der),
                }]
            },
            "tlogEntries": [{
                "logIndex": signed_bundle.rekor_bundle.payload.log_index,
                "logId": {
                    "keyId": base64::engine::general_purpose::STANDARD.encode(log_id),
                },
                "kindVersion": {
                    "kind": kind,
                    "version": api_version,
                },
                "integratedTime": signed_bundle.rekor_bundle.payload.integrated_time,
                "inclusionPromise": {
                    "signedEntryTimestamp": base64::engine::general_purpose::STANDARD.encode(signed_entry_timestamp),
                },
                "canonicalizedBody": base64::engine::general_purpose::STANDARD.encode(canonicalized_body),
            }]
        },
        "messageSignature": {
            "messageDigest": {
                "algorithm": "SHA2_256",
                "digest": base64::engine::general_purpose::STANDARD.encode(digest),
            },
            "signature": base64::engine::general_purpose::STANDARD.encode(signature_bytes),
        }
    });

    serde_json::from_value(bundle_json).map_err(|e| {
        eyre::eyre!(
            "Failed to construct Sigstore bundle from policy artifacts: {}",
            e
        )
    })
}

/// The image profile this node expects its KMS's policy to be for, from
/// `MERO_TEE_PROFILE`. Unset means no profile check.
fn expected_profile_from_env() -> Option<String> {
    std::env::var("MERO_TEE_PROFILE")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Parse a release policy and check it is the file this node asked for.
///
/// Every release's policy is validly signed, so a signature says a file is SOME
/// release's policy, not that it is this one's. The URL names the release, but
/// the fields inside are what the signature covers, so they are checked too.
fn parse_policy_json_for_release(
    json_str: &str,
    version: &str,
    expected_profile: Option<&str>,
) -> EyreResult<KmsAttestationPolicy> {
    let root: PolicyJson =
        serde_json::from_str(json_str).map_err(|e| eyre::eyre!("Invalid policy JSON: {}", e))?;
    if let Some(tag) = root.tag.as_deref() {
        let tag = tag.trim().strip_prefix("mero-kms-v").unwrap_or(tag.trim());
        if tag != version {
            bail!("Policy JSON is for release {tag}, not the requested {version}");
        }
    }
    if let Some(expected) = expected_profile {
        match root.profile.as_deref() {
            Some(profile) if profile == expected => {}
            Some(profile) => bail!(
                "Policy JSON is for the {profile} profile, but this node expects {expected} \
                 (MERO_TEE_PROFILE)"
            ),
            None => bail!(
                "Policy JSON names no profile, but this node expects {expected} (MERO_TEE_PROFILE)"
            ),
        }
    }
    policy_from_root(root)
}

#[cfg(test)]
fn parse_policy_json(json_str: &str) -> EyreResult<KmsAttestationPolicy> {
    let root: PolicyJson =
        serde_json::from_str(json_str).map_err(|e| eyre::eyre!("Invalid policy JSON: {}", e))?;
    policy_from_root(root)
}

/// The KMS allowlist under its published name, else under the older one.
fn kms_allowlist(prefixed: Vec<String>, legacy: Vec<String>) -> Vec<String> {
    if prefixed.is_empty() {
        legacy
    } else {
        prefixed
    }
}

fn policy_from_root(root: PolicyJson) -> EyreResult<KmsAttestationPolicy> {
    if let Some(role) = root.role.as_deref() {
        if role != "kms" {
            bail!("Policy JSON has role {role:?}; a KMS release policy has role \"kms\"");
        }
    }
    let PolicySection {
        kms_allowed_tcb_statuses,
        kms_allowed_mrtd,
        kms_allowed_rtmr0,
        kms_allowed_rtmr1,
        kms_allowed_rtmr2,
        kms_allowed_rtmr3,
        allowed_tcb_statuses,
        allowed_mrtd,
        allowed_rtmr0,
        allowed_rtmr1,
        allowed_rtmr2,
        allowed_rtmr3,
    } = root.policy;
    let tcb_statuses = kms_allowlist(kms_allowed_tcb_statuses, allowed_tcb_statuses);

    let allowed_tcb_statuses: Vec<String> = if tcb_statuses.is_empty() {
        // Default to UpToDate when not specified, matching production hardening
        // expectations for Intel TDX attestation status.
        DEFAULT_ALLOWED_TCB_STATUSES
            .iter()
            .map(|status| (*status).to_owned())
            .collect()
    } else {
        tcb_statuses
            .into_iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let allowed_mrtd = parse_hex_array(&kms_allowlist(kms_allowed_mrtd, allowed_mrtd), 48)?;
    let allowed_rtmr0 = parse_hex_array(&kms_allowlist(kms_allowed_rtmr0, allowed_rtmr0), 48)?;
    let allowed_rtmr1 = parse_hex_array(&kms_allowlist(kms_allowed_rtmr1, allowed_rtmr1), 48)?;
    let allowed_rtmr2 = parse_hex_array(&kms_allowlist(kms_allowed_rtmr2, allowed_rtmr2), 48)?;
    let allowed_rtmr3 = parse_hex_array(&kms_allowlist(kms_allowed_rtmr3, allowed_rtmr3), 48)?;

    if allowed_tcb_statuses.is_empty() {
        bail!(
            "Policy JSON missing policy.kms_allowed_tcb_statuses (or policy.allowed_tcb_statuses) (at least one TCB status is required)"
        );
    }
    if allowed_mrtd.is_empty() {
        bail!("Policy JSON missing policy.kms_allowed_mrtd (or policy.allowed_mrtd) (at least one MRTD value is required)");
    }
    if allowed_rtmr0.is_empty() {
        bail!("Policy JSON missing policy.kms_allowed_rtmr0 (or policy.allowed_rtmr0) (at least one RTMR0 value is required)");
    }
    if allowed_rtmr1.is_empty() {
        bail!("Policy JSON missing policy.kms_allowed_rtmr1 (or policy.allowed_rtmr1) (at least one RTMR1 value is required)");
    }
    if allowed_rtmr2.is_empty() {
        bail!("Policy JSON missing policy.kms_allowed_rtmr2 (or policy.allowed_rtmr2) (at least one RTMR2 value is required)");
    }
    if allowed_rtmr3.is_empty() {
        bail!("Policy JSON missing policy.kms_allowed_rtmr3 (or policy.allowed_rtmr3) (at least one RTMR3 value is required)");
    }

    let default_binding_b64 = root.kms.default_binding_b64.trim().to_string();
    if default_binding_b64.is_empty() {
        bail!("Policy JSON missing kms.default_binding_b64");
    }
    let decoded_binding = base64::engine::general_purpose::STANDARD
        .decode(&default_binding_b64)
        .map_err(|e| {
            eyre::eyre!(
                "Policy JSON kms.default_binding_b64 is invalid base64: {}",
                e
            )
        })?;
    if decoded_binding.len() != 32 {
        bail!(
            "Policy JSON kms.default_binding_b64 must decode to exactly 32 bytes, got {}",
            decoded_binding.len()
        );
    }

    Ok(KmsAttestationPolicy {
        allowed_tcb_statuses,
        allowed_mrtd,
        allowed_rtmr0,
        allowed_rtmr1,
        allowed_rtmr2,
        allowed_rtmr3,
        default_binding_b64,
    })
}

fn parse_hex_array(values: &[String], expected_bytes: usize) -> EyreResult<Vec<String>> {
    let mut parsed = Vec::with_capacity(values.len());
    for (i, v) in values.iter().enumerate() {
        let trimmed = v.trim();
        let normalized = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed)
            .to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        let bytes = hex::decode(&normalized)
            .map_err(|e| eyre::eyre!("Policy value[{}] invalid hex: {}", i, e))?;
        if bytes.len() != expected_bytes {
            bail!(
                "Policy value[{}] invalid length: expected {} bytes, got {}",
                i,
                expected_bytes,
                bytes.len()
            );
        }
        parsed.push(normalized);
    }
    Ok(parsed)
}

fn normalize_release_version(raw: &str) -> EyreResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("release version cannot be empty");
    }

    let version = trimmed.strip_prefix("mero-kms-v").unwrap_or(trimmed);
    if !is_valid_release_version(version) {
        bail!(
            "release version must be semver-like (e.g. 2.1.14 or 2.1.14-rc.1), got '{}'",
            trimmed
        );
    }

    Ok(version.to_owned())
}

fn is_valid_release_version(version: &str) -> bool {
    let mut core_and_suffix = version.splitn(2, ['-', '+']);
    let core = core_and_suffix.next().unwrap_or_default();
    let suffix = core_and_suffix.next();

    let mut core_segments = core.split('.');
    let major = core_segments.next();
    let minor = core_segments.next();
    let patch = core_segments.next();
    if core_segments.next().is_some() {
        return false;
    }

    let Some(major) = major else {
        return false;
    };
    let Some(minor) = minor else {
        return false;
    };
    let Some(patch) = patch else {
        return false;
    };

    if major.is_empty() || minor.is_empty() || patch.is_empty() {
        return false;
    }
    if !major.chars().all(|c| c.is_ascii_digit())
        || !minor.chars().all(|c| c.is_ascii_digit())
        || !patch.chars().all(|c| c.is_ascii_digit())
    {
        return false;
    }

    if let Some(suffix) = suffix {
        if suffix.is_empty() {
            return false;
        }
        if !suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
        {
            return false;
        }
    }

    true
}

/// Refuse a release older than `minimum` (`MERO_TEE_MIN_VERSION`).
///
/// The release to verify the KMS against arrives from outside the TD (instance
/// metadata, on the node image), and every release's policy is validly signed.
/// Without a floor, naming an old release whose allowlists accept a KMS build
/// since found wanting is a downgrade anyone who sets the metadata can perform.
/// The image bakes its own version in as the floor: the KMS release that
/// admits an image is always cut after that image was measured.
fn enforce_minimum_release(version: &str, minimum: Option<&str>) -> EyreResult<()> {
    let Some(minimum) = minimum.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(());
    };
    let minimum = normalize_release_version(minimum)
        .map_err(|e| eyre::eyre!("MERO_TEE_MIN_VERSION is invalid: {e}"))?;
    let parse = |v: &str| {
        semver::Version::parse(v).map_err(|e| eyre::eyre!("cannot compare release {v}: {e}"))
    };
    if parse(version)? < parse(&minimum)? {
        bail!(
            "release {version} is older than this node's minimum {minimum} \
             (MERO_TEE_MIN_VERSION); refusing to verify the KMS against it"
        );
    }
    Ok(())
}

fn policy_fetch_backoff(attempt: usize) -> std::time::Duration {
    let exponent = u32::try_from(attempt).unwrap_or(u32::MAX).saturating_sub(1);
    // `1 << exponent` panics once `exponent >= 64` (attempts past ~64); fall back
    // to u64::MAX so the saturating_mul below just clamps to the max backoff.
    let factor = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    let millis = 250_u64
        .saturating_mul(factor)
        .min(POLICY_FETCH_MAX_BACKOFF_MS);
    std::time::Duration::from_millis(millis)
}

/// Resolve policy: fetch from release when version is set, else None.
pub async fn resolve_policy() -> EyreResult<Option<KmsAttestationPolicy>> {
    if use_env_policy() {
        warn!("USE_ENV_POLICY=true: skipping release fetch, using config.toml policy");
        return Ok(None);
    }
    let Some(version) = release_version_from_env()? else {
        return Ok(None);
    };
    let minimum = std::env::var("MERO_TEE_MIN_VERSION").ok();
    enforce_minimum_release(&version, minimum.as_deref())?;

    // Security fail-closed: if operator explicitly configured a release version,
    // we must not continue without attestation policy verification.
    let policy = fetch_policy_from_release(&version).await.map_err(|e| {
        eyre::eyre!(
            "Failed to fetch policy from release mero-kms-v{}: {}",
            version,
            e
        )
    })?;

    info!(
        "Loaded KMS attestation policy from release mero-kms-v{}",
        version
    );
    Ok(Some(policy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use sigstore::cosign::bundle::{Bundle as RekorBundle, Payload as RekorPayload};

    #[test]
    fn policy_fetch_backoff_grows_then_caps() {
        // The first attempts grow exponentially from the 250ms base.
        assert_eq!(policy_fetch_backoff(1).as_millis(), 250);
        assert_eq!(policy_fetch_backoff(2).as_millis(), 500);
        assert_eq!(policy_fetch_backoff(4).as_millis(), 2000);

        // The `.min(cap)` engages well before any shift-width concern: attempt 8
        // (250 * 128 = 32s) is the last value under the 60s ceiling, and attempt
        // 9 (250 * 256 = 64s) is the first clamped to it.
        let cap = u128::from(POLICY_FETCH_MAX_BACKOFF_MS);
        assert_eq!(policy_fetch_backoff(8).as_millis(), 32_000);
        assert_eq!(policy_fetch_backoff(9).as_millis(), cap);

        // Extreme attempts stay clamped: attempt 65 (exponent 64) is the exact
        // u64 shift-width boundary where `checked_shl` returns None, so the
        // `checked_shl`/`try_from` guards must keep it at the ceiling, not panic.
        assert_eq!(policy_fetch_backoff(65).as_millis(), cap);
        assert_eq!(policy_fetch_backoff(usize::MAX).as_millis(), cap);
    }

    #[test]
    fn release_version_from_env_uses_priority_order() {
        let resolved = release_version_from_env_with(|env_var| match env_var {
            "MERO_KMS_RELEASE_TAG" => Some("mero-kms-v2.1.12".to_owned()),
            "MERO_KMS_VERSION" => Some("2.1.11".to_owned()),
            "MERO_TEE_VERSION" => Some("2.1.10".to_owned()),
            _ => None,
        })
        .expect("version resolution should succeed");
        assert_eq!(resolved.as_deref(), Some("2.1.12"));
    }

    #[test]
    fn release_version_from_env_falls_back_when_high_priority_empty() {
        let resolved = release_version_from_env_with(|env_var| match env_var {
            "MERO_KMS_RELEASE_TAG" => Some("   ".to_owned()),
            "MERO_KMS_VERSION" => Some("2.1.11".to_owned()),
            "MERO_TEE_VERSION" => Some("2.1.10".to_owned()),
            _ => None,
        })
        .expect("empty high-priority env var should be skipped");
        assert_eq!(resolved.as_deref(), Some("2.1.11"));
    }

    #[test]
    fn release_version_from_env_rejects_invalid_value() {
        let err = release_version_from_env_with(|env_var| match env_var {
            "MERO_KMS_VERSION" => Some("../malicious".to_owned()),
            _ => None,
        })
        .expect_err("invalid release version should fail")
        .to_string();
        assert!(err.contains("MERO_KMS_VERSION is invalid"));
    }

    #[test]
    fn release_version_from_env_uses_fallback_paths() {
        let resolved_from_kms = release_version_from_env_with(|env_var| match env_var {
            "MERO_KMS_VERSION" => Some("2.1.22".to_owned()),
            _ => None,
        })
        .expect("kms fallback should resolve");
        assert_eq!(resolved_from_kms.as_deref(), Some("2.1.22"));

        let resolved_from_tee = release_version_from_env_with(|env_var| match env_var {
            "MERO_TEE_VERSION" => Some("2.1.23".to_owned()),
            _ => None,
        })
        .expect("tee fallback should resolve");
        assert_eq!(resolved_from_tee.as_deref(), Some("2.1.23"));
    }

    #[test]
    fn parse_policy_json_requires_non_empty_mrtd_allowlist() {
        let json = r#"{
            "policy": {
                "allowed_tcb_statuses": ["UpToDate"],
                "allowed_mrtd": []
            },
            "kms": {
                "default_binding_b64": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
            }
        }"#;

        let err = parse_policy_json(json)
            .expect_err("empty MRTD allowlist should fail")
            .to_string();
        assert!(err.contains("policy.allowed_mrtd"));
    }

    #[test]
    fn parse_policy_json_accepts_valid_policy() {
        let json = format!(
            r#"{{
                "policy": {{
                    "allowed_tcb_statuses": ["UpToDate"],
                    "allowed_mrtd": ["{mrtd}"],
                    "allowed_rtmr0": ["{rtmr0}"],
                    "allowed_rtmr1": ["{rtmr1}"],
                    "allowed_rtmr2": ["{rtmr2}"],
                    "allowed_rtmr3": ["{rtmr3}"]
                }},
                "kms": {{
                    "default_binding_b64": "{binding}"
                }}
            }}"#,
            mrtd = "ab".repeat(48),
            rtmr0 = "cd".repeat(48),
            rtmr1 = "ef".repeat(48),
            rtmr2 = "12".repeat(48),
            rtmr3 = "34".repeat(48),
            binding = base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        );

        let policy = parse_policy_json(&json).expect("policy should parse");
        assert_eq!(policy.allowed_tcb_statuses, vec!["uptodate".to_owned()]);
        assert_eq!(policy.allowed_mrtd, vec!["ab".repeat(48)]);
        assert_eq!(policy.allowed_rtmr0, vec!["cd".repeat(48)]);
        assert_eq!(policy.allowed_rtmr1, vec!["ef".repeat(48)]);
        assert_eq!(policy.allowed_rtmr2, vec!["12".repeat(48)]);
        assert_eq!(policy.allowed_rtmr3, vec!["34".repeat(48)]);
    }

    #[test]
    fn parse_policy_json_rejects_invalid_binding_length() {
        let json = format!(
            r#"{{
                "policy": {{
                    "allowed_tcb_statuses": ["UpToDate"],
                    "allowed_mrtd": ["{mrtd}"],
                    "allowed_rtmr0": ["{rtmr0}"],
                    "allowed_rtmr1": ["{rtmr1}"],
                    "allowed_rtmr2": ["{rtmr2}"],
                    "allowed_rtmr3": ["{rtmr3}"]
                }},
                "kms": {{
                    "default_binding_b64": "{binding}"
                }}
            }}"#,
            mrtd = "ab".repeat(48),
            rtmr0 = "cd".repeat(48),
            rtmr1 = "ef".repeat(48),
            rtmr2 = "12".repeat(48),
            rtmr3 = "34".repeat(48),
            binding = base64::engine::general_purpose::STANDARD.encode([7u8; 31]),
        );

        let err = parse_policy_json(&json)
            .expect_err("invalid binding size should fail")
            .to_string();
        assert!(err.contains("must decode to exactly 32 bytes"));
    }

    #[test]
    fn parse_policy_json_requires_non_empty_rtmr_allowlists() {
        let json = format!(
            r#"{{
                "policy": {{
                    "allowed_tcb_statuses": ["UpToDate"],
                    "allowed_mrtd": ["{mrtd}"],
                    "allowed_rtmr0": [],
                    "allowed_rtmr1": ["{rtmr1}"],
                    "allowed_rtmr2": ["{rtmr2}"],
                    "allowed_rtmr3": ["{rtmr3}"]
                }},
                "kms": {{
                    "default_binding_b64": "{binding}"
                }}
            }}"#,
            mrtd = "ab".repeat(48),
            rtmr1 = "ef".repeat(48),
            rtmr2 = "12".repeat(48),
            rtmr3 = "34".repeat(48),
            binding = base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        );

        let err = parse_policy_json(&json)
            .expect_err("empty RTMR allowlist should fail")
            .to_string();
        assert!(err.contains("policy.allowed_rtmr0"));
    }

    #[test]
    fn parse_hex_array_accepts_uppercase_prefix() {
        let values = vec![format!("0X{}", "CD".repeat(48))];
        let parsed = parse_hex_array(&values, 48).expect("0X prefix should be accepted");
        assert_eq!(parsed, vec!["cd".repeat(48)]);
    }

    fn make_signed_artifact_bundle(log_id: &str) -> SignedArtifactBundle {
        let cert_pem = "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n";
        SignedArtifactBundle {
            base64_signature: base64::engine::general_purpose::STANDARD.encode([7u8; 64]),
            cert: base64::engine::general_purpose::STANDARD.encode(cert_pem),
            rekor_bundle: RekorBundle {
                signed_entry_timestamp: base64::engine::general_purpose::STANDARD.encode([8u8; 64]),
                payload: RekorPayload {
                    body: base64::engine::general_purpose::STANDARD
                        .encode(br#"{"apiVersion":"0.0.1","kind":"hashedrekord"}"#),
                    integrated_time: 1,
                    log_index: 1,
                    log_id: log_id.to_owned(),
                },
            },
        }
    }

    #[test]
    fn decode_bundle_certificate_pem_rejects_invalid_base64() {
        let err = decode_bundle_certificate_pem("!!not-base64!!")
            .expect_err("invalid certificate encoding must fail");
        assert!(err.to_string().contains("not valid base64"));
    }

    #[test]
    fn build_policy_sigstore_bundle_rejects_invalid_log_id() {
        let signed_bundle = make_signed_artifact_bundle("not-a-hex-log-id");
        let err = build_policy_sigstore_bundle(
            b"{}",
            &signed_bundle,
            "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n",
        )
        .expect_err("invalid log ID must fail");
        assert!(err.to_string().contains("log ID"));
    }

    /// A policy exactly as `Release mero-kms` publishes it. The parser used to
    /// read unprefixed `allowed_*` names that no published file carries, so
    /// every real policy failed to parse and `init --kms-url` could not succeed.
    const PUBLISHED_POLICY: &str =
        include_str!("../testdata/kms-phala-attestation-policy-2.3.69.json");

    #[test]
    fn a_published_policy_parses_to_its_kms_allowlists() {
        let raw: serde_json::Value = serde_json::from_str(PUBLISHED_POLICY).unwrap();
        let policy = parse_policy_json_for_release(PUBLISHED_POLICY, "2.3.69", None)
            .expect("the published layout must parse");

        let kms_mrtd = raw["policy"]["kms_allowed_mrtd"][0].as_str().unwrap();
        let node_mrtd = raw["policy"]["node_allowed_mrtd"][0].as_str().unwrap();
        assert_eq!(policy.allowed_mrtd, vec![kms_mrtd.to_owned()]);
        // The node allowlists describe the nodes a KMS serves, not the KMS.
        assert!(!policy.allowed_mrtd.contains(&node_mrtd.to_owned()));
        assert_eq!(policy.allowed_tcb_statuses, vec!["uptodate", "outofdate"]);
    }

    #[test]
    fn a_policy_for_another_release_or_profile_is_refused() {
        let err = parse_policy_json_for_release(PUBLISHED_POLICY, "2.3.70", None)
            .expect_err("a file from another release must not stand in for this one");
        assert!(
            err.to_string().contains("not the requested 2.3.70"),
            "{err}"
        );

        assert!(parse_policy_json_for_release(
            PUBLISHED_POLICY,
            "2.3.69",
            Some("locked-read-only")
        )
        .is_ok());
        let err = parse_policy_json_for_release(PUBLISHED_POLICY, "2.3.69", Some("debug"))
            .expect_err("a locked-profile policy must not verify a debug node's KMS");
        assert!(err.to_string().contains("MERO_TEE_PROFILE"), "{err}");
    }

    #[test]
    fn a_policy_for_something_other_than_a_kms_is_refused() {
        let mut raw: serde_json::Value = serde_json::from_str(PUBLISHED_POLICY).unwrap();
        raw["role"] = serde_json::json!("node");
        let err = parse_policy_json_for_release(&raw.to_string(), "2.3.69", None)
            .expect_err("only a KMS policy verifies a KMS");
        assert!(err.to_string().contains("role"), "{err}");
    }

    #[test]
    fn a_release_below_the_minimum_is_refused() {
        enforce_minimum_release("2.3.69", None).unwrap();
        enforce_minimum_release("2.3.69", Some("  ")).unwrap();
        enforce_minimum_release("2.3.69", Some("2.3.69")).unwrap();
        enforce_minimum_release("2.3.70", Some("mero-kms-v2.3.69")).unwrap();
        // Numeric, not lexical: 2.3.100 is newer than 2.3.69.
        enforce_minimum_release("2.3.100", Some("2.3.69")).unwrap();

        let err = enforce_minimum_release("2.3.7", Some("2.3.69"))
            .expect_err("an older release must not be accepted");
        assert!(err.to_string().contains("MERO_TEE_MIN_VERSION"), "{err}");
        assert!(enforce_minimum_release("2.3.69", Some("not-a-version")).is_err());
    }

    /// The per-profile fixtures: the published locked-read-only policy, and the
    /// same file as the release would publish it for `debug-read-only` — its own
    /// profile and its own KMS measurements.
    fn per_profile_policies() -> (String, String) {
        let mut debug: serde_json::Value = serde_json::from_str(PUBLISHED_POLICY).unwrap();
        debug["profile"] = serde_json::json!("debug-read-only");
        debug["policy"]["kms_allowed_mrtd"] = serde_json::json!(["dd".repeat(48)]);
        (PUBLISHED_POLICY.to_owned(), debug.to_string())
    }

    #[test]
    fn a_profile_asks_for_its_own_policy_asset_first() {
        let generic = PolicyAssets {
            json: "kms-phala-attestation-policy.json".to_owned(),
            signature: "kms-phala-attestation-policy.json.sig".to_owned(),
            bundle: "kms-phala-attestation-policy.json.bundle.json".to_owned(),
        };
        assert_eq!(policy_asset_candidates(None).unwrap(), vec![generic]);

        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let names: Vec<_> = candidates
            .iter()
            .map(|a| (a.json.as_str(), a.signature.as_str(), a.bundle.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                (
                    "kms-phala-attestation-policy.debug-read-only.json",
                    "kms-phala-attestation-policy.debug-read-only.json.sig",
                    "kms-phala-attestation-policy.debug-read-only.json.bundle.json",
                ),
                (
                    "kms-phala-attestation-policy.json",
                    "kms-phala-attestation-policy.json.sig",
                    "kms-phala-attestation-policy.json.bundle.json",
                ),
            ]
        );

        for bad in ["../locked", "Debug", "debug/x", "-x", "debug read"] {
            let err = policy_asset_candidates(Some(bad)).expect_err(bad);
            assert!(err.to_string().contains("MERO_TEE_PROFILE"), "{err}");
        }
    }

    #[test]
    fn each_profile_verifies_only_against_its_own_policy() {
        let (locked, debug) = per_profile_policies();

        let policy = parse_policy_json_for_release(&debug, "2.3.69", Some("debug-read-only"))
            .expect("a debug node must accept its own profile's policy");
        assert_eq!(policy.allowed_mrtd, vec!["dd".repeat(48)]);
        assert!(parse_policy_json_for_release(&locked, "2.3.69", Some("locked-read-only")).is_ok());

        // Neither profile's policy stands in for the other's.
        assert!(parse_policy_json_for_release(&locked, "2.3.69", Some("debug-read-only")).is_err());
        assert!(parse_policy_json_for_release(&debug, "2.3.69", Some("locked-read-only")).is_err());
    }

    /// Serve `assets` (name -> body) under `/<tag>/`; anything else is a 404.
    async fn spawn_release_server(
        assets: Vec<(&'static str, String)>,
    ) -> (String, reqwest::Client) {
        use axum::extract::Path;
        use axum::http::StatusCode;
        use axum::routing::get;

        let assets: std::collections::HashMap<_, _> = assets.into_iter().collect();
        let app = axum::Router::new().route(
            "/{tag}/{asset}",
            get(move |Path((_tag, asset)): Path<(String, String)>| {
                let body = assets.get(asset.as_str()).cloned();
                async move { body.ok_or(StatusCode::NOT_FOUND) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener should have an addr");
        drop(tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("release test server should run");
        }));
        let client = reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("client should build");
        (format!("http://{addr}"), client)
    }

    #[tokio::test]
    async fn a_debug_node_fetches_the_debug_policy_not_the_locked_one() {
        let (locked, debug) = per_profile_policies();
        let (base, client) = spawn_release_server(vec![
            ("kms-phala-attestation-policy.json", locked),
            (
                "kms-phala-attestation-policy.json.sig",
                "locked-sig".to_owned(),
            ),
            (
                "kms-phala-attestation-policy.json.bundle.json",
                "locked-bundle".to_owned(),
            ),
            (
                "kms-phala-attestation-policy.debug-read-only.json",
                debug.clone(),
            ),
            (
                "kms-phala-attestation-policy.debug-read-only.json.sig",
                "debug-sig".to_owned(),
            ),
            (
                "kms-phala-attestation-policy.debug-read-only.json.bundle.json",
                "debug-bundle".to_owned(),
            ),
        ])
        .await;

        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let (assets, bodies) =
            fetch_first_published_policy(&client, &base, "mero-kms-v2.3.69", &candidates)
                .await
                .expect("the debug policy is published");
        assert_eq!(
            assets.json,
            "kms-phala-attestation-policy.debug-read-only.json"
        );
        assert_eq!(bodies.policy, debug);
        assert_eq!(bodies.signature, "debug-sig");
        assert_eq!(bodies.bundle, "debug-bundle");
    }

    #[tokio::test]
    async fn a_release_without_per_profile_assets_falls_back_to_the_generic_one() {
        let (locked, _) = per_profile_policies();
        let (base, client) = spawn_release_server(vec![
            ("kms-phala-attestation-policy.json", locked.clone()),
            (
                "kms-phala-attestation-policy.json.sig",
                "locked-sig".to_owned(),
            ),
            (
                "kms-phala-attestation-policy.json.bundle.json",
                "locked-bundle".to_owned(),
            ),
        ])
        .await;

        let candidates = policy_asset_candidates(Some("locked-read-only")).unwrap();
        let (assets, bodies) =
            fetch_first_published_policy(&client, &base, "mero-kms-v2.3.69", &candidates)
                .await
                .expect("an older release still serves locked nodes");
        assert_eq!(assets.json, "kms-phala-attestation-policy.json");
        assert!(
            parse_policy_json_for_release(&bodies.policy, "2.3.69", Some("locked-read-only"))
                .is_ok()
        );

        // A debug node may also land on the generic file, but it is refused.
        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let (_, bodies) =
            fetch_first_published_policy(&client, &base, "mero-kms-v2.3.69", &candidates)
                .await
                .unwrap();
        assert!(
            parse_policy_json_for_release(&bodies.policy, "2.3.69", Some("debug-read-only"))
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_published_policy_missing_its_signature_is_an_error_not_a_fallback() {
        let (locked, debug) = per_profile_policies();
        let (base, client) = spawn_release_server(vec![
            ("kms-phala-attestation-policy.json", locked),
            (
                "kms-phala-attestation-policy.json.sig",
                "locked-sig".to_owned(),
            ),
            (
                "kms-phala-attestation-policy.json.bundle.json",
                "locked-bundle".to_owned(),
            ),
            ("kms-phala-attestation-policy.debug-read-only.json", debug),
        ])
        .await;

        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let err = fetch_first_published_policy(&client, &base, "mero-kms-v2.3.69", &candidates)
            .await
            .err()
            .expect("a half-published policy must not fall through to another file");
        assert!(
            err.to_string().contains(".debug-read-only.json.sig"),
            "{err}"
        );

        let (base, client) = spawn_release_server(vec![]).await;
        let err = fetch_first_published_policy(&client, &base, "mero-kms-v2.3.69", &candidates)
            .await
            .err()
            .expect("a release with no policy at all is an error");
        assert!(
            err.to_string().contains("none of the policy assets"),
            "{err}"
        );
    }
}
