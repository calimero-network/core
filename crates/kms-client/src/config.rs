//! The KMS settings a TEE node fetches its keys with (v1's `[tee.kms]`): where
//! the KMS is and how to reach it, never what to trust it on. A KMS is trusted
//! only through the signed release policy (`crate::policy`).

use camino::Utf8PathBuf;
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
    /// The KMS's own attestation is verified against the release policy
    /// (`crate::policy::resolve_policy`), so no allowlists are needed here.
    #[must_use]
    pub fn kms(url: Url) -> Self {
        Self {
            kms: Some(KmsConfig {
                url,
                tls: KmsTlsConfig::default(),
            }),
        }
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
    // An `attestation` table (v1's `[tee.kms.attestation]` allowlists) still
    // parses and is ignored: a KMS is trusted only through the signed release
    // policy. Test: `an_attestation_table_parses_and_pins_nothing`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A `[tee.kms]` table written for an earlier release, with allowlists
    /// under `attestation`, still loads; the allowlists go nowhere.
    #[test]
    fn an_attestation_table_parses_and_pins_nothing() {
        let kms: KmsConfig = serde_json::from_value(serde_json::json!({
            "url": "http://kms.example.com/",
            "attestation": {
                "enabled": true,
                "accept_mock": false,
                "allowed_mrtd": ["aa".repeat(48)],
                "policy_json_path": "/etc/calimero/kms-policy.json"
            }
        }))
        .expect("an old [tee.kms] table still parses");
        assert_eq!(kms.url.as_str(), "http://kms.example.com/");
    }
}
