//! KMS attestation policy for merod.
//!
//! When MERO_KMS_RELEASE_TAG, MERO_KMS_VERSION, or MERO_TEE_VERSION is set,
//! merod fetches the attestation policy from the official mero-tee release
//! instead of relying on config written by external scripts.
//! Use USE_ENV_POLICY=true for air-gapped deployments
//! (requires policy in config.toml via apply-merod-kms-phala-attestation-config.sh).

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
const DEFAULT_ALLOWED_TCB_STATUSES: &[&str] = &["uptodate"];
const POLICY_JSON_ASSET: &str = "kms-phala-attestation-policy.json";
const POLICY_SIG_ASSET: &str = "kms-phala-attestation-policy.json.sig";
const POLICY_BUNDLE_ASSET: &str = "kms-phala-attestation-policy.json.bundle.json";
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
    #[serde(default)]
    policy: PolicySection,
    #[serde(default)]
    kms: KmsSection,
}

#[derive(Debug, Deserialize, Default)]
struct PolicySection {
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
pub async fn fetch_policy_from_release(version: &str) -> EyreResult<KmsAttestationPolicy> {
    let version = normalize_release_version(version)?;
    let tag = format!("mero-kms-v{}", version);
    let policy_url = format!("{}/{}/{}", POLICY_RELEASE_BASE, tag, POLICY_JSON_ASSET);
    let signature_url = format!("{}/{}/{}", POLICY_RELEASE_BASE, tag, POLICY_SIG_ASSET);
    let bundle_url = format!("{}/{}/{}", POLICY_RELEASE_BASE, tag, POLICY_BUNDLE_ASSET);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("merod/1.0")
        .build()
        .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;
    let mut last_error = String::new();
    for attempt in 1..=POLICY_FETCH_RETRIES {
        let policy_body = match fetch_release_asset(&client, &policy_url, POLICY_JSON_ASSET).await {
            AssetFetchResult::Success(body) => body,
            AssetFetchResult::Transient(error) => {
                last_error = error;
                if attempt < POLICY_FETCH_RETRIES {
                    warn!(
                        attempt,
                        retries = POLICY_FETCH_RETRIES,
                        error = %last_error,
                        "Transient policy fetch status, retrying"
                    );
                    tokio::time::sleep(policy_fetch_backoff(attempt)).await;
                    continue;
                }
                break;
            }
            AssetFetchResult::Permanent(error) => {
                last_error = error;
                break;
            }
        };
        let signature_body =
            match fetch_release_asset(&client, &signature_url, POLICY_SIG_ASSET).await {
                AssetFetchResult::Success(body) => body,
                AssetFetchResult::Transient(error) => {
                    last_error = error;
                    if attempt < POLICY_FETCH_RETRIES {
                        warn!(
                            attempt,
                            retries = POLICY_FETCH_RETRIES,
                            error = %last_error,
                            "Transient policy signature fetch status, retrying"
                        );
                        tokio::time::sleep(policy_fetch_backoff(attempt)).await;
                        continue;
                    }
                    break;
                }
                AssetFetchResult::Permanent(error) => {
                    last_error = error;
                    break;
                }
            };
        let bundle_body = match fetch_release_asset(&client, &bundle_url, POLICY_BUNDLE_ASSET).await
        {
            AssetFetchResult::Success(body) => body,
            AssetFetchResult::Transient(error) => {
                last_error = error;
                if attempt < POLICY_FETCH_RETRIES {
                    warn!(
                        attempt,
                        retries = POLICY_FETCH_RETRIES,
                        error = %last_error,
                        "Transient policy bundle fetch status, retrying"
                    );
                    tokio::time::sleep(policy_fetch_backoff(attempt)).await;
                    continue;
                }
                break;
            }
            AssetFetchResult::Permanent(error) => {
                last_error = error;
                break;
            }
        };

        verify_policy_signature(&policy_body, &signature_body, &bundle_body)
            .await
            .map_err(|e| eyre::eyre!("Policy signature verification failed: {}", e))?;

        return parse_policy_json(&policy_body);
    }

    bail!("{last_error}")
}

enum AssetFetchResult {
    Success(String),
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
                "Failed to read {asset_name} response body: {}",
                err
            )),
        },
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
        Err(err) => AssetFetchResult::Transient(format!("Failed to fetch {asset_name}: {}", err)),
    }
}

async fn verify_policy_signature(
    policy_body: &str,
    signature_body: &str,
    bundle_body: &str,
) -> EyreResult<()> {
    let trust_root = SigstoreTrustRoot::new(None)
        .await
        .map_err(|e| eyre::eyre!("Failed to initialize Sigstore trust root: {}", e))?;
    let rekor_pub_keys = rekor_public_keys(&trust_root)?;
    let signed_bundle = SignedArtifactBundle::new_verified(bundle_body, &rekor_pub_keys)
        .map_err(|e| eyre::eyre!("Invalid signed bundle: {}", e))?;

    let detached_signature = signature_body.trim();
    if detached_signature.is_empty() {
        bail!("Policy signature asset is empty");
    }
    if detached_signature != signed_bundle.base64_signature.trim() {
        bail!(
            "Policy signature mismatch between {} and {}",
            POLICY_SIG_ASSET,
            POLICY_BUNDLE_ASSET
        );
    }

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
    let mut hasher = Sha256::new();
    hasher.update(policy_body.as_bytes());
    verifier
        .verify_digest(hasher, policy_bundle, &workflow_policy, true)
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
    let verification_key =
        CosignVerificationKey::try_from(&certificate.tbs_certificate.subject_public_key_info)
            .map_err(|e| {
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

fn parse_policy_json(json_str: &str) -> EyreResult<KmsAttestationPolicy> {
    let root: PolicyJson =
        serde_json::from_str(json_str).map_err(|e| eyre::eyre!("Invalid policy JSON: {}", e))?;

    let allowed_tcb_statuses: Vec<String> = if root.policy.allowed_tcb_statuses.is_empty() {
        // Default to UpToDate when not specified, matching production hardening
        // expectations for Intel TDX attestation status.
        DEFAULT_ALLOWED_TCB_STATUSES
            .iter()
            .map(|status| (*status).to_owned())
            .collect()
    } else {
        root.policy
            .allowed_tcb_statuses
            .into_iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };

    let allowed_mrtd = parse_hex_array(&root.policy.allowed_mrtd, 48)?;
    let allowed_rtmr0 = parse_hex_array(&root.policy.allowed_rtmr0, 48)?;
    let allowed_rtmr1 = parse_hex_array(&root.policy.allowed_rtmr1, 48)?;
    let allowed_rtmr2 = parse_hex_array(&root.policy.allowed_rtmr2, 48)?;
    let allowed_rtmr3 = parse_hex_array(&root.policy.allowed_rtmr3, 48)?;

    if allowed_tcb_statuses.is_empty() {
        bail!(
            "Policy JSON missing policy.allowed_tcb_statuses (at least one TCB status is required)"
        );
    }
    if allowed_mrtd.is_empty() {
        bail!("Policy JSON missing policy.allowed_mrtd (at least one MRTD value is required)");
    }
    if allowed_rtmr0.is_empty() {
        bail!("Policy JSON missing policy.allowed_rtmr0 (at least one RTMR0 value is required)");
    }
    if allowed_rtmr1.is_empty() {
        bail!("Policy JSON missing policy.allowed_rtmr1 (at least one RTMR1 value is required)");
    }
    if allowed_rtmr2.is_empty() {
        bail!("Policy JSON missing policy.allowed_rtmr2 (at least one RTMR2 value is required)");
    }
    if allowed_rtmr3.is_empty() {
        bail!("Policy JSON missing policy.allowed_rtmr3 (at least one RTMR3 value is required)");
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

fn policy_fetch_backoff(attempt: usize) -> std::time::Duration {
    let exponent = (attempt as u32).saturating_sub(1);
    std::time::Duration::from_millis(250_u64.saturating_mul(1_u64 << exponent))
}

/// Resolve policy: fetch from release when version is set, else None.
pub async fn resolve_policy() -> EyreResult<Option<KmsAttestationPolicy>> {
    if use_env_policy() {
        info!("USE_ENV_POLICY=true: skipping release fetch, using config.toml policy");
        return Ok(None);
    }
    let Some(version) = release_version_from_env()? else {
        return Ok(None);
    };

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
}
