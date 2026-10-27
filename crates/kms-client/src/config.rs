//! The KMS settings a TEE node fetches its keys with (v1's `[tee.kms]`).

use camino::Utf8PathBuf;
use eyre::{bail, Result as EyreResult};
use serde::{Deserialize, Serialize};
use url::Url;

/// Configuration for TEE (Trusted Execution Environment) features.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TeeConfig {
    /// KMS that releases this node's storage encryption key (`[tee.kms]`).
    pub kms: Option<KmsConfig>,
}

impl TeeConfig {
    /// A TEE config that fetches its storage key from the KMS at `url`, with
    /// every other setting at its default.
    ///
    /// The shape `merod init --kms-url` writes. The KMS's own attestation is
    /// verified against the release policy merod fetches at startup (see
    /// `merod`'s `kms_policy`), so no allowlists are needed here.
    #[must_use]
    pub fn kms(url: Url) -> Self {
        Self {
            kms: Some(KmsConfig {
                url,
                tls: KmsTlsConfig::default(),
                attestation: KmsAttestationConfig::default(),
            }),
        }
    }

    /// Whether this node is configured to enforce *real* KMS attestation.
    ///
    /// Returns true when the KMS has attestation verification enabled and is
    /// not running in mock-accepting development mode. This is the predicate
    /// used to refuse the dev-only `--mock-tee` flag: mock and real attestation
    /// must be mutually exclusive on a single node.
    ///
    /// Returns `false` when no KMS is configured (`tee.kms` absent), which
    /// permits `--mock-tee`.
    #[must_use]
    pub fn has_real_attestation(&self) -> bool {
        self.kms
            .as_ref()
            .is_some_and(|kms| kms.attestation.enabled && !kms.attestation.accept_mock)
    }
}

/// Configuration for the mero-kms TDX cluster that releases storage keys.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct KmsConfig {
    /// URL of the mero-kms service.
    pub url: Url,
    /// Optional TLS hardening settings for KMS transport.
    #[serde(default)]
    pub tls: KmsTlsConfig,
    /// KMS self-attestation verification policy.
    #[serde(default)]
    pub attestation: KmsAttestationConfig,
}

/// TLS configuration for KMS transport hardening.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct KmsTlsConfig {
    /// Optional PEM-encoded CA certificate path for private trust roots.
    ///
    /// When set, merod adds this certificate to its trust store for KMS TLS.
    /// Path must be absolute and point to an existing file.
    #[serde(default)]
    pub ca_cert_path: Option<Utf8PathBuf>,
    /// Optional PEM-encoded client certificate path for mTLS.
    ///
    /// Must be provided together with `client_key_path`.
    /// Path must be absolute and point to an existing file.
    #[serde(default)]
    pub client_cert_path: Option<Utf8PathBuf>,
    /// Optional PEM-encoded client private key path for mTLS.
    ///
    /// Must be provided together with `client_cert_path`.
    /// Path must be absolute and point to an existing file.
    #[serde(default)]
    pub client_key_path: Option<Utf8PathBuf>,
}

/// Configuration for verifying KMS self-attestation (`POST /attest`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct KmsAttestationConfig {
    /// Enable KMS attestation verification before requesting keys.
    #[serde(default)]
    pub enabled: bool,
    /// Accept mock quotes for development only.
    ///
    /// WARNING: Enabling this in production bypasses real attestation
    /// cryptographic guarantees and weakens the trust model.
    #[serde(default)]
    pub accept_mock: bool,
    /// Allowed TCB statuses for KMS quote verification.
    #[serde(default = "default_kms_attestation_tcb_statuses")]
    pub allowed_tcb_statuses: Vec<String>,
    /// Allowed KMS MRTD values (hex, with or without `0x` prefix).
    #[serde(default)]
    pub allowed_mrtd: Vec<String>,
    /// Allowed KMS RTMR0 values (hex).
    ///
    /// Required when `enabled=true` and `accept_mock=false`.
    #[serde(default)]
    pub allowed_rtmr0: Vec<String>,
    /// Allowed KMS RTMR1 values (hex).
    ///
    /// Required when `enabled=true` and `accept_mock=false`.
    #[serde(default)]
    pub allowed_rtmr1: Vec<String>,
    /// Allowed KMS RTMR2 values (hex).
    ///
    /// Required when `enabled=true` and `accept_mock=false`.
    #[serde(default)]
    pub allowed_rtmr2: Vec<String>,
    /// Allowed KMS RTMR3 values (hex).
    ///
    /// Required when `enabled=true` and `accept_mock=false`.
    #[serde(default)]
    pub allowed_rtmr3: Vec<String>,
    /// Optional base64-encoded 32-byte binding value for `/attest`.
    ///
    /// If unset, merod uses the default domain separator binding.
    #[serde(default)]
    pub binding_b64: Option<String>,
    /// Optional path to externally-generated attestation policy JSON.
    ///
    /// This is intended for deployment startup scripts (e.g. mero-tee image
    /// startup hooks) that fetch and verify signed policy artifacts, then
    /// provide policy allowlists to merod.
    #[serde(default)]
    pub policy_json_path: Option<Utf8PathBuf>,
}

impl Default for KmsAttestationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            accept_mock: false,
            allowed_tcb_statuses: default_kms_attestation_tcb_statuses(),
            allowed_mrtd: Vec::new(),
            allowed_rtmr0: Vec::new(),
            allowed_rtmr1: Vec::new(),
            allowed_rtmr2: Vec::new(),
            allowed_rtmr3: Vec::new(),
            binding_b64: None,
            policy_json_path: None,
        }
    }
}

impl KmsAttestationConfig {
    /// Validate required attestation policy fields when attestation is enabled.
    ///
    /// This keeps startup validation consistent with runtime policy normalization.
    pub fn validate_enabled_policy(&self) -> EyreResult<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.accept_mock {
            // Development mode: production strictness does not apply.
            return Ok(());
        }

        if !has_non_empty_status_allowlist(&self.allowed_tcb_statuses) {
            bail!("tee.kms.attestation.enabled is true, but allowed_tcb_statuses is empty.");
        }

        if !has_non_empty_measurement_allowlist(&self.allowed_mrtd) {
            bail!(
                "tee.kms.attestation.enabled is true, but allowed_mrtd is empty. \
                 Configure at least one trusted KMS MRTD."
            );
        }

        for (field_name, values) in [
            ("allowed_rtmr0", &self.allowed_rtmr0),
            ("allowed_rtmr1", &self.allowed_rtmr1),
            ("allowed_rtmr2", &self.allowed_rtmr2),
            ("allowed_rtmr3", &self.allowed_rtmr3),
        ] {
            if has_non_empty_measurement_allowlist(values) {
                continue;
            }

            bail!(
                "tee.kms.attestation.enabled is true and accept_mock is false, \
                 but {field_name} is empty."
            );
        }

        Ok(())
    }
}

fn has_non_empty_status_allowlist(values: &[String]) -> bool {
    values.iter().any(|status| !status.trim().is_empty())
}

fn has_non_empty_measurement_allowlist(values: &[String]) -> bool {
    values
        .iter()
        .map(|measurement| normalize_attestation_measurement(measurement))
        .any(|measurement| !measurement.is_empty())
}

/// Normalize a configured attestation measurement value for comparison.
///
/// Trims surrounding whitespace, strips a leading `0x` prefix if present, and
/// lowercases ASCII hex characters.
pub fn normalize_attestation_measurement(value: &str) -> String {
    let normalized = value.trim().to_ascii_lowercase();
    normalized
        .strip_prefix("0x")
        .unwrap_or(normalized.as_str())
        .to_owned()
}

fn default_kms_attestation_tcb_statuses() -> Vec<String> {
    vec!["UpToDate".to_owned()]
}
