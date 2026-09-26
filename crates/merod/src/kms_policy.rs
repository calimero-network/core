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
use calimero_tee_release::{fetch_verified_asset_if_published, KMS_RELEASE_IDENTITY};
use eyre::{bail, Result as EyreResult};
use serde::Deserialize;
use tracing::{info, warn};

const DEFAULT_ALLOWED_TCB_STATUSES: &[&str] = &["uptodate"];
/// Release assets are `<stem>.json` (generic) and `<stem>.<profile>.json`
/// (per image profile), each with a `.sig` and a `.bundle.json` beside it.
const POLICY_ASSET_STEM: &str = "kms-phala-attestation-policy";

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
    /// Released KMS compose hashes (hex, lowercase, no 0x prefix), from
    /// `policy.kms_allowed_event_payload`. The KMS's RTMR3 event log must
    /// measure one of these (mero-tee#338).
    pub allowed_compose_hashes: Vec<String>,
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
    /// The compose hashes of the released KMS compose files. The name is the
    /// release workflow's: the hash is the payload of the RTMR3 `compose-hash`
    /// event. No older layout carries it.
    #[serde(default)]
    kms_allowed_event_payload: Vec<String>,
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
    let tag = format!("mero-kms-v{version}");
    let expected_profile = expected_profile_from_env();
    let candidates = policy_asset_candidates(expected_profile.as_deref())?;
    let (asset, policy_body) = fetch_first_published(&tag, &candidates, |asset| {
        fetch_verified_asset_if_published(&tag, asset, &KMS_RELEASE_IDENTITY)
    })
    .await
    .map_err(|e| eyre::eyre!("Policy fetch or signature verification failed: {}", e))?;
    info!(%asset, "Verified KMS attestation policy signature");
    parse_policy_json_for_release(&policy_body, &version, expected_profile.as_deref())
}

/// The policy assets to try, in order.
///
/// A node with a profile asks for that profile's policy first. The generic
/// asset follows it only as a fallback for releases that publish no per-profile
/// file: it is the locked-read-only policy, so on any other profile its
/// `profile` field fails the check in [`parse_policy_json_for_release`] rather
/// than admitting a KMS with the wrong measurements.
fn policy_asset_candidates(profile: Option<&str>) -> EyreResult<Vec<String>> {
    let generic = format!("{POLICY_ASSET_STEM}.json");
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
    Ok(vec![format!("{POLICY_ASSET_STEM}.{profile}.json"), generic])
}

/// Return the first candidate the release publishes, with its verified body.
///
/// `fetch` answers `Ok(None)` only when the release has no such asset; every
/// other failure (a missing signature, a bad one, an outage) is returned as is
/// and never moves on to the next candidate.
async fn fetch_first_published<'a, F, Fut>(
    tag: &str,
    candidates: &'a [String],
    mut fetch: F,
) -> EyreResult<(&'a str, String)>
where
    F: FnMut(&'a str) -> Fut,
    Fut: std::future::Future<Output = EyreResult<Option<String>>>,
{
    for (index, asset) in candidates.iter().enumerate() {
        let Some(body) = fetch(asset).await? else {
            warn!(release = tag, %asset, "Release publishes no such policy asset");
            continue;
        };
        if index > 0 {
            warn!(
                release = tag,
                %asset,
                "Falling back to the generic policy; its profile must still match MERO_TEE_PROFILE"
            );
        }
        return Ok((asset, body));
    }
    bail!(
        "Release {tag} publishes none of the policy assets {}",
        candidates.join(", ")
    )
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
pub(crate) fn parse_policy_json(json_str: &str) -> EyreResult<KmsAttestationPolicy> {
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
        kms_allowed_event_payload,
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
    let allowed_compose_hashes = parse_hex_array(&kms_allowed_event_payload, 32)?;

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
    // Registers alone do not pin the KMS app: its owner can upgrade it to a new
    // compose file, and that file decides who can derive node keys.
    if allowed_compose_hashes.is_empty() {
        bail!("Policy JSON missing policy.kms_allowed_event_payload (at least one released KMS compose hash is required)");
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
        allowed_compose_hashes,
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
    calimero_tee_release::normalize_release_version(raw, "mero-kms-v")
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
                    "allowed_rtmr3": ["{rtmr3}"],
                    "kms_allowed_event_payload": ["{compose}"]
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
            compose = "56".repeat(32),
            binding = base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        );

        let policy = parse_policy_json(&json).expect("policy should parse");
        assert_eq!(policy.allowed_tcb_statuses, vec!["uptodate".to_owned()]);
        assert_eq!(policy.allowed_mrtd, vec!["ab".repeat(48)]);
        assert_eq!(policy.allowed_rtmr0, vec!["cd".repeat(48)]);
        assert_eq!(policy.allowed_rtmr1, vec!["ef".repeat(48)]);
        assert_eq!(policy.allowed_rtmr2, vec!["12".repeat(48)]);
        assert_eq!(policy.allowed_rtmr3, vec!["34".repeat(48)]);
        assert_eq!(policy.allowed_compose_hashes, vec!["56".repeat(32)]);
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
                    "allowed_rtmr3": ["{rtmr3}"],
                    "kms_allowed_event_payload": ["{compose}"]
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
            compose = "56".repeat(32),
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
                    "allowed_rtmr3": ["{rtmr3}"],
                    "kms_allowed_event_payload": ["{compose}"]
                }},
                "kms": {{
                    "default_binding_b64": "{binding}"
                }}
            }}"#,
            mrtd = "ab".repeat(48),
            rtmr1 = "ef".repeat(48),
            rtmr2 = "12".repeat(48),
            rtmr3 = "34".repeat(48),
            compose = "56".repeat(32),
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
        let compose = raw["policy"]["kms_allowed_event_payload"][0]
            .as_str()
            .unwrap();
        assert_eq!(policy.allowed_compose_hashes, vec![compose.to_owned()]);
    }

    #[test]
    fn a_policy_that_pins_no_compose_hash_is_refused() {
        let mut raw: serde_json::Value = serde_json::from_str(PUBLISHED_POLICY).unwrap();
        raw["policy"]["kms_allowed_event_payload"] = serde_json::json!([]);
        let err = parse_policy_json_for_release(&raw.to_string(), "2.3.69", None)
            .expect_err("registers alone must not pin the KMS app");
        assert!(
            err.to_string().contains("kms_allowed_event_payload"),
            "{err}"
        );
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
        assert_eq!(
            policy_asset_candidates(None).unwrap(),
            vec!["kms-phala-attestation-policy.json"]
        );
        assert_eq!(
            policy_asset_candidates(Some("debug-read-only")).unwrap(),
            vec![
                "kms-phala-attestation-policy.debug-read-only.json",
                "kms-phala-attestation-policy.json",
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

    /// A release as `fetch_first_published` sees it: each asset is published
    /// (`Ok(Some)`), absent (`Ok(None)`), or fails (`Err`).
    fn release(
        assets: &[(&'static str, Result<&str, &'static str>)],
    ) -> impl FnMut(&str) -> std::future::Ready<EyreResult<Option<String>>> {
        let assets: std::collections::HashMap<&'static str, Result<String, &'static str>> = assets
            .iter()
            .map(|(name, body)| (*name, body.map(str::to_owned)))
            .collect();
        move |asset| {
            std::future::ready(match assets.get(asset) {
                None => Ok(None),
                Some(Ok(body)) => Ok(Some(body.clone())),
                Some(Err(error)) => Err(eyre::eyre!("{error}")),
            })
        }
    }

    #[tokio::test]
    async fn a_debug_node_fetches_the_debug_policy_not_the_locked_one() {
        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let (asset, body) = fetch_first_published(
            "mero-kms-v2.3.69",
            &candidates,
            release(&[
                ("kms-phala-attestation-policy.json", Ok("locked")),
                (
                    "kms-phala-attestation-policy.debug-read-only.json",
                    Ok("debug"),
                ),
            ]),
        )
        .await
        .expect("the debug policy is published");
        assert_eq!(asset, "kms-phala-attestation-policy.debug-read-only.json");
        assert_eq!(body, "debug");
    }

    #[tokio::test]
    async fn a_release_without_per_profile_assets_falls_back_to_the_generic_one() {
        let (locked, _) = per_profile_policies();
        let published = [("kms-phala-attestation-policy.json", Ok(locked.as_str()))];

        let candidates = policy_asset_candidates(Some("locked-read-only")).unwrap();
        let (asset, body) =
            fetch_first_published("mero-kms-v2.3.69", &candidates, release(&published))
                .await
                .expect("an older release still serves locked nodes");
        assert_eq!(asset, "kms-phala-attestation-policy.json");
        assert!(parse_policy_json_for_release(&body, "2.3.69", Some("locked-read-only")).is_ok());

        // A debug node may also land on the generic file, but it is refused.
        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let (_, body) = fetch_first_published("mero-kms-v2.3.69", &candidates, release(&published))
            .await
            .unwrap();
        assert!(parse_policy_json_for_release(&body, "2.3.69", Some("debug-read-only")).is_err());
    }

    #[tokio::test]
    async fn a_failed_per_profile_fetch_is_an_error_not_a_fallback() {
        let candidates = policy_asset_candidates(Some("debug-read-only")).unwrap();
        let err = fetch_first_published(
            "mero-kms-v2.3.69",
            &candidates,
            release(&[
                ("kms-phala-attestation-policy.json", Ok("locked")),
                (
                    "kms-phala-attestation-policy.debug-read-only.json",
                    Err("signature verification failed"),
                ),
            ]),
        )
        .await
        .expect_err("a published but unverifiable policy must not fall through to another file");
        assert!(
            err.to_string().contains("signature verification failed"),
            "{err}"
        );

        let err = fetch_first_published("mero-kms-v2.3.69", &candidates, release(&[]))
            .await
            .expect_err("a release with no policy at all is an error");
        assert!(
            err.to_string().contains("none of the policy assets"),
            "{err}"
        );
    }
}
