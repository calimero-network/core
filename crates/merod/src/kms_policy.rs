//! KMS attestation policy for merod.
//!
//! When MERO_TEE_VERSION or MERO_KMS_VERSION is set, merod fetches the attestation
//! policy from the official mero-tee release instead of relying on config written
//! by external scripts. Use USE_ENV_POLICY=true for air-gapped deployments
//! (requires policy in config.toml via apply-merod-kms-phala-attestation-config.sh).

use eyre::{bail, Result as EyreResult};
use serde::Deserialize;
use tracing::info;

const POLICY_RELEASE_BASE: &str =
    "https://github.com/calimero-network/mero-tee/releases/download";

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

/// Read version from MERO_TEE_VERSION or MERO_KMS_VERSION.
pub fn release_version_from_env() -> Option<String> {
    let tag = std::env::var("MERO_KMS_RELEASE_TAG").ok();
    let version = std::env::var("MERO_KMS_VERSION").ok();
    let tee_version = std::env::var("MERO_TEE_VERSION").ok();
    let v = tag.or(version).or(tee_version)?;
    let s = v.trim();
    Some(if s.starts_with("mero-kms-v") {
        s.strip_prefix("mero-kms-v").unwrap_or(s).to_string()
    } else {
        s.to_string()
    })
}

/// Whether to skip release fetch and use config.toml policy (air-gapped).
pub fn use_env_policy() -> bool {
    std::env::var("USE_ENV_POLICY")
        .map(|v| matches!(v.trim().to_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Fetch attestation policy from the official mero-tee release.
pub async fn fetch_policy_from_release(version: &str) -> EyreResult<KmsAttestationPolicy> {
    let tag = format!("mero-kms-v{}", version.trim());
    let url = format!(
        "{}/{}/kms-phala-attestation-policy.json",
        POLICY_RELEASE_BASE, tag
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("merod/1.0")
        .build()
        .map_err(|e| eyre::eyre!("Failed to create HTTP client: {}", e))?;
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| eyre::eyre!("Policy fetch failed: {}", e))?;
    if !resp.status().is_success() {
        bail!("Policy fetch failed: {} {}", resp.status(), url);
    }
    let body = resp
        .text()
        .await
        .map_err(|e| eyre::eyre!("Failed to read policy response: {}", e))?;
    parse_policy_json(&body)
}

fn parse_policy_json(json_str: &str) -> EyreResult<KmsAttestationPolicy> {
    let root: PolicyJson = serde_json::from_str(json_str)
        .map_err(|e| eyre::eyre!("Invalid policy JSON: {}", e))?;

    let allowed_tcb_statuses = if root.policy.allowed_tcb_statuses.is_empty() {
        vec!["uptodate".to_owned()]
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

    let default_binding_b64 = root.kms.default_binding_b64.trim().to_string();
    if default_binding_b64.is_empty() {
        bail!("Policy JSON missing kms.default_binding_b64");
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
        let normalized = v.trim().trim_start_matches("0x").to_ascii_lowercase();
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

/// Resolve policy: fetch from release when version is set, else None.
pub async fn resolve_policy() -> EyreResult<Option<KmsAttestationPolicy>> {
    if use_env_policy() {
        info!("USE_ENV_POLICY=true: skipping release fetch, using config.toml policy");
        return Ok(None);
    }
    let Some(version) = release_version_from_env() else {
        return Ok(None);
    };
    match fetch_policy_from_release(&version).await {
        Ok(policy) => {
            info!(
                "Loaded KMS attestation policy from release mero-kms-v{}",
                version
            );
            Ok(Some(policy))
        }
        Err(e) => {
            tracing::warn!(
                "Failed to fetch policy from release ({}): {}. Proceeding without KMS verification.",
                version,
                e
            );
            Ok(None)
        }
    }
}
