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
use tracing::{info, warn};

const POLICY_RELEASE_BASE: &str = "https://github.com/calimero-network/mero-tee/releases/download";
const POLICY_FETCH_RETRIES: usize = 3;
const DEFAULT_ALLOWED_TCB_STATUSES: &[&str] = &["uptodate"];

/// Attestation policy for KMS verification (mirrors mero-kms AttestationPolicy).
#[derive(Debug, Clone)]
pub struct KmsAttestationPolicy {
    /// Allowed TCB statuses (normalized to lowercase).
    pub allowed_tcb_statuses: Vec<String>,
    /// Allowed MRTD values (hex, lowercase, no 0x prefix).
    pub allowed_mrtd: Vec<String>,
    /// Optional allowed RTMR0-3 values (hex, lowercase, no 0x prefix).
    /// Empty lists intentionally skip the corresponding RTMR checks.
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
/// Trust model: this currently relies on HTTPS transport and GitHub release
/// access controls. Deployment tooling may enforce stronger artifact signature
/// verification upstream.
pub async fn fetch_policy_from_release(version: &str) -> EyreResult<KmsAttestationPolicy> {
    let version = normalize_release_version(version)?;
    let tag = format!("mero-kms-v{}", version);
    let url = format!(
        "{}/{}/kms-phala-attestation-policy.json",
        POLICY_RELEASE_BASE, tag
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("merod/1.0")
        .build()
        .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;
    let mut last_error = String::new();
    for attempt in 1..=POLICY_FETCH_RETRIES {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let body = resp
                    .text()
                    .await
                    .map_err(|e| eyre::eyre!("Failed to read policy response: {}", e))?;
                return parse_policy_json(&body);
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                last_error = format!("Policy fetch failed: {} {} {}", status, url, body);

                if (status.is_server_error() || status.as_u16() == 429)
                    && attempt < POLICY_FETCH_RETRIES
                {
                    warn!(
                        attempt,
                        retries = POLICY_FETCH_RETRIES,
                        %status,
                        "Transient policy fetch status, retrying"
                    );
                    tokio::time::sleep(policy_fetch_backoff(attempt)).await;
                    continue;
                }

                break;
            }
            Err(err) => {
                last_error = format!("Policy fetch failed: {}", err);
                if attempt < POLICY_FETCH_RETRIES {
                    warn!(
                        attempt,
                        retries = POLICY_FETCH_RETRIES,
                        error = %err,
                        "Transient policy fetch error, retrying"
                    );
                    tokio::time::sleep(policy_fetch_backoff(attempt)).await;
                    continue;
                }
                break;
            }
        }
    }

    bail!("{last_error}")
}

fn parse_policy_json(json_str: &str) -> EyreResult<KmsAttestationPolicy> {
    let root: PolicyJson =
        serde_json::from_str(json_str).map_err(|e| eyre::eyre!("Invalid policy JSON: {}", e))?;

    let allowed_tcb_statuses = if root.policy.allowed_tcb_statuses.is_empty() {
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

    if allowed_mrtd.is_empty() {
        bail!("Policy JSON missing policy.allowed_mrtd (at least one MRTD value is required)");
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
                    "allowed_rtmr0": [],
                    "allowed_rtmr1": [],
                    "allowed_rtmr2": [],
                    "allowed_rtmr3": []
                }},
                "kms": {{
                    "default_binding_b64": "{binding}"
                }}
            }}"#,
            mrtd = "ab".repeat(48),
            binding = base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        );

        let policy = parse_policy_json(&json).expect("policy should parse");
        assert_eq!(policy.allowed_tcb_statuses, vec!["uptodate".to_owned()]);
        assert_eq!(policy.allowed_mrtd, vec!["ab".repeat(48)]);
    }

    #[test]
    fn parse_policy_json_rejects_invalid_binding_length() {
        let json = format!(
            r#"{{
                "policy": {{
                    "allowed_tcb_statuses": ["UpToDate"],
                    "allowed_mrtd": ["{mrtd}"]
                }},
                "kms": {{
                    "default_binding_b64": "{binding}"
                }}
            }}"#,
            mrtd = "ab".repeat(48),
            binding = base64::engine::general_purpose::STANDARD.encode([7u8; 31]),
        );

        let err = parse_policy_json(&json)
            .expect_err("invalid binding size should fail")
            .to_string();
        assert!(err.contains("must decode to exactly 32 bytes"));
    }

    #[test]
    fn parse_hex_array_accepts_uppercase_prefix() {
        let values = vec![format!("0X{}", "CD".repeat(48))];
        let parsed = parse_hex_array(&values, 48).expect("0X prefix should be accepted");
        assert_eq!(parsed, vec!["cd".repeat(48)]);
    }
}
