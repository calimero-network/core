use core::time::Duration;

use calimero_context::config::ContextConfig;
use calimero_network_primitives::config::{BootstrapConfig, DiscoveryConfig, SwarmConfig};
use calimero_server::admin::service::AdminConfig;
use calimero_server::config::AuthMode;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::sse::SseConfig;
use calimero_server::ws::WsConfig;
use camino::{Utf8Path, Utf8PathBuf};
use eyre::{bail, Result as EyreResult, WrapErr};
use multiaddr::Multiaddr;
use serde::{Deserialize, Serialize};
use tokio::fs::{read_to_string, write};
use url::Url;

use mero_auth::config::AuthConfig;

pub use calimero_node_primitives::NodeMode;

pub const CONFIG_FILE: &str = "config.toml";

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ConfigFile {
    #[serde(
        with = "serde_identity",
        default = "libp2p_identity::Keypair::generate_ed25519"
    )]
    pub identity: libp2p_identity::Keypair,

    #[serde(default)]
    pub mode: NodeMode,

    #[serde(flatten)]
    pub network: NetworkConfig,

    pub sync: SyncConfig,

    pub datastore: DataStoreConfig,

    pub blobstore: BlobStoreConfig,

    pub context: ContextConfig,

    /// TEE-related configuration (KMS, attestation, etc.).
    #[serde(default)]
    pub tee: Option<TeeConfig>,
}

/// Configuration for TEE (Trusted Execution Environment) features.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct TeeConfig {
    /// KMS configuration for fetching storage encryption keys.
    pub kms: KmsConfig,
}

/// Configuration for the Key Management Service.
///
/// Supports multiple KMS implementations. Currently only Phala is supported.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct KmsConfig {
    /// Phala Cloud KMS configuration (mero-kms-phala).
    pub phala: Option<PhalaKmsConfig>,
}

/// Configuration for Phala Cloud KMS (mero-kms-phala).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct PhalaKmsConfig {
    /// URL of the mero-kms-phala service.
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
    #[serde(default)]
    pub ca_cert_path: Option<Utf8PathBuf>,
    /// Optional PEM-encoded client certificate path for mTLS.
    ///
    /// Must be provided together with `client_key_path`.
    #[serde(default)]
    pub client_cert_path: Option<Utf8PathBuf>,
    /// Optional PEM-encoded client private key path for mTLS.
    ///
    /// Must be provided together with `client_cert_path`.
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

        let has_tcb_status = self
            .allowed_tcb_statuses
            .iter()
            .map(|status| status.trim())
            .any(|status| !status.is_empty());
        if !has_tcb_status {
            bail!("tee.kms.phala.attestation.enabled is true, but allowed_tcb_statuses is empty.");
        }

        let has_mrtd = self
            .allowed_mrtd
            .iter()
            .map(|measurement| normalize_attestation_measurement(measurement))
            .any(|measurement| !measurement.is_empty());
        if !has_mrtd {
            bail!(
                "tee.kms.phala.attestation.enabled is true, but allowed_mrtd is empty. \
                 Configure at least one trusted KMS MRTD."
            );
        }

        let has_rtmr0 = self
            .allowed_rtmr0
            .iter()
            .map(|measurement| normalize_attestation_measurement(measurement))
            .any(|measurement| !measurement.is_empty());
        if !has_rtmr0 {
            bail!(
                "tee.kms.phala.attestation.enabled is true and accept_mock is false, \
                 but allowed_rtmr0 is empty."
            );
        }

        let has_rtmr1 = self
            .allowed_rtmr1
            .iter()
            .map(|measurement| normalize_attestation_measurement(measurement))
            .any(|measurement| !measurement.is_empty());
        if !has_rtmr1 {
            bail!(
                "tee.kms.phala.attestation.enabled is true and accept_mock is false, \
                 but allowed_rtmr1 is empty."
            );
        }

        let has_rtmr2 = self
            .allowed_rtmr2
            .iter()
            .map(|measurement| normalize_attestation_measurement(measurement))
            .any(|measurement| !measurement.is_empty());
        if !has_rtmr2 {
            bail!(
                "tee.kms.phala.attestation.enabled is true and accept_mock is false, \
                 but allowed_rtmr2 is empty."
            );
        }

        let has_rtmr3 = self
            .allowed_rtmr3
            .iter()
            .map(|measurement| normalize_attestation_measurement(measurement))
            .any(|measurement| !measurement.is_empty());
        if !has_rtmr3 {
            bail!(
                "tee.kms.phala.attestation.enabled is true and accept_mock is false, \
                 but allowed_rtmr3 is empty."
            );
        }

        Ok(())
    }
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

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(rename = "timeout_ms", with = "serde_duration")]
    pub timeout: Duration,
    #[serde(rename = "interval_ms", with = "serde_duration")]
    pub interval: Duration,
    #[serde(rename = "frequency_ms", with = "serde_duration")]
    pub frequency: Duration,
}

/// Configuration for specialized node functionality (e.g., read-only nodes).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[non_exhaustive]
pub struct SpecializedNodeConfig {
    /// Topic name for specialized node invite discovery messages.
    #[serde(default = "default_specialized_node_invite_topic")]
    pub invite_topic: String,

    /// Whether to accept mock TEE attestation.
    /// WARNING: Should only be true for testing. Never enable in production!
    #[serde(default)]
    pub accept_mock_tee: bool,
}

fn default_specialized_node_invite_topic() -> String {
    "mero_specialized_node_invites".to_owned()
}

impl Default for SpecializedNodeConfig {
    fn default() -> Self {
        Self {
            invite_topic: default_specialized_node_invite_topic(),
            accept_mock_tee: false,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct NetworkConfig {
    pub swarm: SwarmConfig,

    pub server: ServerConfig,

    #[serde(default)]
    pub bootstrap: BootstrapConfig,

    #[serde(default)]
    pub discovery: DiscoveryConfig,

    /// Configuration for specialized nodes (read-only, etc.).
    #[serde(default)]
    pub specialized_node: SpecializedNodeConfig,
}

impl NetworkConfig {
    #[must_use]
    pub fn new(
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
        server: ServerConfig,
    ) -> Self {
        Self {
            swarm,
            server,
            bootstrap,
            discovery,
            specialized_node: SpecializedNodeConfig::default(),
        }
    }

    /// Create a new `NetworkConfig` with custom specialized node settings.
    #[must_use]
    pub fn with_specialized_node(
        swarm: SwarmConfig,
        bootstrap: BootstrapConfig,
        discovery: DiscoveryConfig,
        server: ServerConfig,
        specialized_node: SpecializedNodeConfig,
    ) -> Self {
        Self {
            swarm,
            server,
            bootstrap,
            discovery,
            specialized_node,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ServerConfig {
    pub listen: Vec<Multiaddr>,

    #[serde(default)]
    pub admin: Option<AdminConfig>,

    #[serde(default)]
    pub jsonrpc: Option<JsonRpcConfig>,

    #[serde(default)]
    pub websocket: Option<WsConfig>,

    #[serde(default)]
    pub sse: Option<SseConfig>,

    #[serde(default)]
    pub auth_mode: AuthMode,

    #[serde(default)]
    pub embedded_auth: Option<AuthConfig>,
}

impl ServerConfig {
    #[must_use]
    pub const fn new(
        listen: Vec<Multiaddr>,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
    ) -> Self {
        Self {
            listen,
            admin,
            jsonrpc,
            websocket,
            sse,
            auth_mode: AuthMode::Proxy,
            embedded_auth: None,
        }
    }

    #[must_use]
    pub const fn with_auth(
        listen: Vec<Multiaddr>,
        admin: Option<AdminConfig>,
        jsonrpc: Option<JsonRpcConfig>,
        websocket: Option<WsConfig>,
        sse: Option<SseConfig>,
        auth_mode: AuthMode,
        embedded_auth: Option<AuthConfig>,
    ) -> Self {
        Self {
            listen,
            admin,
            jsonrpc,
            websocket,
            sse,
            auth_mode,
            embedded_auth,
        }
    }

    #[must_use]
    pub fn embedded_auth(&self) -> Option<&AuthConfig> {
        self.embedded_auth.as_ref()
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct DataStoreConfig {
    pub path: Utf8PathBuf,
}

impl DataStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct BlobStoreConfig {
    pub path: Utf8PathBuf,
}

impl BlobStoreConfig {
    #[must_use]
    pub const fn new(path: Utf8PathBuf) -> Self {
        Self { path }
    }
}

impl ConfigFile {
    #[must_use]
    pub const fn new(
        identity: libp2p_identity::Keypair,
        mode: NodeMode,
        network: NetworkConfig,
        sync: SyncConfig,
        datastore: DataStoreConfig,
        blobstore: BlobStoreConfig,
        context: ContextConfig,
    ) -> Self {
        Self {
            identity,
            mode,
            network,
            sync,
            datastore,
            blobstore,
            context,
            tee: None,
        }
    }

    #[must_use]
    pub fn exists(dir: &Utf8Path) -> bool {
        dir.join(CONFIG_FILE).is_file()
    }

    pub async fn load(dir: &Utf8Path) -> EyreResult<Self> {
        let path = dir.join(CONFIG_FILE);
        let content = read_to_string(&path).await.wrap_err_with(|| {
            format!(
                "failed to read configuration from {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        toml::from_str(&content).map_err(Into::into)
    }

    pub async fn save(&self, dir: &Utf8Path) -> EyreResult<()> {
        let path = dir.join(CONFIG_FILE);
        let content = toml::to_string_pretty(self)?;

        write(&path, content).await.wrap_err_with(|| {
            format!(
                "failed to write configuration to {:?}",
                dir.join(CONFIG_FILE)
            )
        })?;

        Ok(())
    }
}

mod serde_duration {
    use core::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(duration: &Duration, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(duration.as_millis() as u64)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        u64::deserialize(deserializer).map(Duration::from_millis)
    }
}

pub mod serde_identity {
    use core::fmt::{self, Formatter};

    use libp2p_identity::Keypair;
    use serde::de::{self, MapAccess};
    use serde::ser::{self, SerializeMap};
    use serde::{Deserializer, Serializer};

    pub fn serialize<S>(key: &Keypair, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut keypair = serializer.serialize_map(Some(2))?;
        keypair.serialize_entry("peer_id", &key.public().to_peer_id().to_base58())?;
        keypair.serialize_entry(
            "keypair",
            &bs58::encode(&key.to_protobuf_encoding().map_err(ser::Error::custom)?).into_string(),
        )?;
        keypair.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Keypair, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IdentityVisitor;

        impl<'de> de::Visitor<'de> for IdentityVisitor {
            type Value = Keypair;

            fn expecting(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
                formatter.write_str("an identity")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut peer_id = None::<String>;
                let mut priv_key = None::<String>;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "peer_id" => peer_id = Some(map.next_value()?),
                        "keypair" => priv_key = Some(map.next_value()?),
                        _ => {
                            drop(map.next_value::<de::IgnoredAny>());
                        }
                    }
                }

                let peer_id = peer_id.ok_or_else(|| de::Error::missing_field("peer_id"))?;
                let priv_key = priv_key.ok_or_else(|| de::Error::missing_field("keypair"))?;

                let priv_key = bs58::decode(priv_key)
                    .into_vec()
                    .map_err(|_| de::Error::custom("invalid base58"))?;

                let keypair = Keypair::from_protobuf_encoding(&priv_key)
                    .map_err(|_| de::Error::custom("invalid protobuf"))?;

                if peer_id != keypair.public().to_peer_id().to_base58() {
                    return Err(de::Error::custom("Peer ID does not match public key"));
                }

                Ok(keypair)
            }
        }

        deserializer.deserialize_struct("Keypair", &["peer_id", "keypair"], IdentityVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_attestation_measurement, KmsAttestationConfig};

    fn make_strict_production_config() -> KmsAttestationConfig {
        KmsAttestationConfig {
            enabled: true,
            accept_mock: false,
            allowed_tcb_statuses: vec!["UpToDate".to_owned()],
            allowed_mrtd: vec!["ab".repeat(48)],
            allowed_rtmr0: vec!["ab".repeat(48)],
            allowed_rtmr1: vec!["ab".repeat(48)],
            allowed_rtmr2: vec!["ab".repeat(48)],
            allowed_rtmr3: vec!["ab".repeat(48)],
            ..KmsAttestationConfig::default()
        }
    }

    #[test]
    fn validate_enabled_policy_allows_disabled_config() {
        let cfg = KmsAttestationConfig::default();
        assert!(cfg.validate_enabled_policy().is_ok());
    }

    #[test]
    fn validate_enabled_policy_rejects_empty_tcb_statuses() {
        let mut cfg = KmsAttestationConfig {
            enabled: true,
            ..KmsAttestationConfig::default()
        };
        cfg.allowed_tcb_statuses = vec!["   ".to_owned()];
        cfg.allowed_mrtd = vec!["ab".repeat(48)];

        let err = cfg.validate_enabled_policy().unwrap_err().to_string();
        assert!(err.contains("allowed_tcb_statuses is empty"));
    }

    #[test]
    fn validate_enabled_policy_rejects_empty_mrtd() {
        let mut cfg = make_strict_production_config();
        cfg.allowed_mrtd = vec!["  ".to_owned(), "0x".to_owned()];

        let err = cfg.validate_enabled_policy().unwrap_err().to_string();
        assert!(err.contains("allowed_mrtd is empty"));
    }

    #[test]
    fn validate_enabled_policy_rejects_empty_any_required_rtmr_allowlist() {
        let mut cfg = make_strict_production_config();
        cfg.allowed_rtmr0.clear();
        let err = cfg.validate_enabled_policy().unwrap_err().to_string();
        assert!(err.contains("allowed_rtmr0 is empty"));

        let mut cfg = make_strict_production_config();
        cfg.allowed_rtmr1.clear();
        let err = cfg.validate_enabled_policy().unwrap_err().to_string();
        assert!(err.contains("allowed_rtmr1 is empty"));

        let mut cfg = make_strict_production_config();
        cfg.allowed_rtmr2.clear();
        let err = cfg.validate_enabled_policy().unwrap_err().to_string();
        assert!(err.contains("allowed_rtmr2 is empty"));

        let mut cfg = make_strict_production_config();
        cfg.allowed_rtmr3.clear();
        let err = cfg.validate_enabled_policy().unwrap_err().to_string();
        assert!(err.contains("allowed_rtmr3 is empty"));
    }

    #[test]
    fn validate_enabled_policy_allows_empty_allowlists_when_accept_mock_is_true() {
        let mut cfg = KmsAttestationConfig {
            enabled: true,
            accept_mock: true,
            ..KmsAttestationConfig::default()
        };
        cfg.allowed_tcb_statuses.clear();
        cfg.allowed_mrtd.clear();
        cfg.allowed_rtmr0.clear();
        cfg.allowed_rtmr1.clear();
        cfg.allowed_rtmr2.clear();
        cfg.allowed_rtmr3.clear();

        assert!(cfg.validate_enabled_policy().is_ok());
    }

    #[test]
    fn normalize_attestation_measurement_handles_uppercase_prefix() {
        assert_eq!(
            normalize_attestation_measurement(" 0XABCD "),
            "abcd".to_owned()
        );
    }
}
