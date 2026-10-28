//! The node's configuration, `{home}/{node}/config.toml` (v1's place for it).
//!
//! One set of fields serves as `init`'s, `config`'s and `run`'s flags and as
//! the file: every field is optional, so a flag given overrides the file
//! ([`NodeArgs::over`]) and a flag left out keeps it. Environment variables
//! (`MEROD_*`) count as given flags, so a container can run without a file.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use calimero_node::system::Authority;
use clap::Args;
use serde::{Deserialize, Serialize};

pub fn path(home: &Path) -> PathBuf {
    home.join("config.toml")
}

pub fn read(file: &Path) -> eyre::Result<NodeArgs> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| eyre::eyre!("reading {}: {e}", file.display()))?;
    toml::from_str(&text).map_err(|e| eyre::eyre!("{}: {e}", file.display()))
}

/// Write `args` to `file`, readable by its owner only: it may hold the ops
/// token or a development storage key.
pub fn write(file: &Path, args: &NodeArgs) -> eyre::Result<()> {
    let text = toml::to_string_pretty(args)?;
    let mut options = std::fs::OpenOptions::new();
    let _ = options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = options.mode(0o600);
    }
    std::io::Write::write_all(&mut options.open(file)?, text.as_bytes())?;
    Ok(())
}

fn authority(s: &str) -> Result<Authority, String> {
    match s {
        "owner" => Ok(Authority::Owner),
        "operator" => Ok(Authority::Operator),
        other => Err(format!("{other:?}: expected owner or operator")),
    }
}

#[derive(Clone, Debug, Default, Args, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct NodeArgs {
    /// This node's id in the cluster.
    #[arg(long, env = "MEROD_NODE_ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<u64>,
    /// Address to listen on [default: 0.0.0.0:2528].
    #[arg(long, env = "MEROD_LISTEN")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<SocketAddr>,
    /// Base URL peers and clients use for this node.
    #[arg(long, env = "MEROD_PUBLIC_URL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
    /// Where the node keeps its state [default: {home}/{node}/data].
    #[arg(long, env = "MEROD_DATA_DIR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<PathBuf>,
    /// Cluster members as `id=url`, comma separated (this node included).
    #[arg(long, env = "MEROD_PEERS", value_delimiter = ',')]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub peers: Vec<String>,
    /// Form the system group with `--peers` as voters (first start).
    #[arg(long, env = "MEROD_BOOTSTRAP", num_args = 0..=1, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap: Option<bool>,
    /// The cluster's name [default: dev].
    #[arg(long, env = "MEROD_CLUSTER")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cluster: Option<String>,
    /// Development mode: mock quotes, a dev storage key, plain requests allowed.
    #[arg(long, env = "MEROD_DEV", num_args = 0..=1, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev: Option<bool>,
    /// Storage key (hex; development only). In production it comes from mero-kms after attestation.
    #[arg(long, env = "MEROD_STORAGE_KEY")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_key: Option<String>,
    /// The passkey relying party id [default: localhost].
    #[arg(long, env = "MEROD_RP_ID")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rp_id: Option<String>,
    /// Origins allowed to run passkey ceremonies and call the API from a
    /// browser [default: http://localhost:3000].
    #[arg(long, env = "MEROD_ORIGINS", value_delimiter = ',')]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<String>,
    /// The Ops API token.
    #[arg(long, env = "MEROD_OPS_TOKEN", hide_env_values = true)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_token: Option<String>,
    /// The ops token's hex SHA-256 instead of the token: the machine's
    /// configuration then holds nothing an operator can sign in with.
    #[arg(long, env = "MEROD_OPS_TOKEN_SHA256", conflicts_with = "ops_token")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ops_token_sha256: Option<String>,
    /// A plain-HTTP address serving only `GET /metrics` (Prometheus text, the
    /// counts `/ops/metrics` answers), for a scraper that cannot open the sealed
    /// channel; bind it to a private address. Off by default.
    #[arg(long, env = "MEROD_METRICS_LISTEN")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics_listen: Option<std::net::SocketAddr>,
    /// How many nodes hold each app [default: 3].
    #[arg(long, env = "MEROD_REPLICAS")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replicas: Option<usize>,
    #[arg(long, env = "MEROD_BACKUP_DIR")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup_dir: Option<PathBuf>,
    /// Rotate the session-signing key after this many seconds (at least 1800:
    /// the previous key must outlive the 15-minute sessions it signed) [default: 21600].
    #[arg(long, env = "MEROD_SESSION_KEY_ROTATION")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_key_rotation_secs: Option<u64>,
    /// A file holding the storage key material (development only).
    #[arg(long, env = "MEROD_STORAGE_KEY_FILE", conflicts_with_all = ["storage_key", "kms_url"])]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_key_file: Option<PathBuf>,
    /// Fetch the storage key from mero-kms (needs `--kms-identity`); the only source in TDX mode.
    #[arg(long, env = "MEROD_KMS_URL", conflicts_with = "storage_key")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kms_url: Option<String>,
    /// The cluster's disk identity (shared by its nodes, so they derive one key).
    #[arg(long, env = "MEROD_KMS_IDENTITY")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kms_identity: Option<PathBuf>,
    /// The mero-tee release this image is (names the signed measurements).
    #[arg(long, env = "MERO_TEE_VERSION")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_release: Option<String>,
    /// The mero-tee image profile this node runs.
    #[arg(long, env = "MERO_TEE_PROFILE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_profile: Option<String>,
    /// Profiles a peer's quote may match (default: this node's own profile).
    #[arg(long, env = "MEROD_PEER_PROFILES", value_delimiter = ',')]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub peer_profiles: Vec<String>,
    /// The oldest mero-tee release a peer may run.
    #[arg(long, env = "MEROD_PEER_MIN_RELEASE")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peer_min_release: Option<String>,
    /// The package registry nodes fetch signed releases from.
    #[arg(long, env = "MEROD_REGISTRY_URL")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_url: Option<String>,
    /// Accept raw WASM uploads, which carry no publisher signature (default: only with --dev).
    #[arg(long, env = "MEROD_ALLOW_UNSIGNED_BUNDLES", num_args = 0..=1, default_missing_value = "true")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allow_unsigned_bundles: Option<bool>,
    /// Who approves the mero-tee releases that may hold the apps' data
    /// (cluster upgrades): `owner` (each app's owner, with a passkey) or
    /// `operator` (for every app) [default: owner]. Fixed when the cluster
    /// first starts and attested with every receipt key, as are the two below.
    #[arg(long, env = "MEROD_RELEASE_APPROVALS", value_parser = authority)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_approvals: Option<Authority>,
    /// Who upgrades apps to another bundle: `owner` or `operator` [default: owner].
    #[arg(long, env = "MEROD_APP_UPGRADES", value_parser = authority)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_upgrades: Option<Authority>,
    /// Who approves the URL prefixes apps may call out to: `owner` or
    /// `operator` [default: owner].
    #[arg(long, env = "MEROD_OUTBOUND_APPROVALS", value_parser = authority)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound_approvals: Option<Authority>,
    /// Serve app frontends from their releases at `{label}.{DOMAIN}`, one
    /// origin per package (a wildcard DNS name pointing at the cluster).
    #[arg(long, env = "MEROD_APPS_DOMAIN")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apps_domain: Option<String>,
    /// A TOML file whose `[outbound]` table replaces the configuration's.
    #[arg(long, env = "MEROD_SETTINGS")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<PathBuf>,
    /// The outbound HTTPS policy (hosts, secrets): the file's `[outbound]`
    /// table. Without one, apps cannot call out.
    #[arg(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outbound: Option<calimero_outbound::OutboundSettings>,
}

impl NodeArgs {
    /// These values, and `base`'s where these leave one out.
    #[must_use]
    pub fn over(self, base: Self) -> Self {
        fn list(a: Vec<String>, b: Vec<String>) -> Vec<String> {
            if a.is_empty() {
                b
            } else {
                a
            }
        }
        Self {
            node_id: self.node_id.or(base.node_id),
            listen: self.listen.or(base.listen),
            public_url: self.public_url.or(base.public_url),
            data_dir: self.data_dir.or(base.data_dir),
            peers: list(self.peers, base.peers),
            bootstrap: self.bootstrap.or(base.bootstrap),
            cluster: self.cluster.or(base.cluster),
            dev: self.dev.or(base.dev),
            storage_key: self.storage_key.or(base.storage_key),
            rp_id: self.rp_id.or(base.rp_id),
            origins: list(self.origins, base.origins),
            ops_token: self.ops_token.or(base.ops_token),
            ops_token_sha256: self.ops_token_sha256.or(base.ops_token_sha256),
            metrics_listen: self.metrics_listen.or(base.metrics_listen),
            replicas: self.replicas.or(base.replicas),
            backup_dir: self.backup_dir.or(base.backup_dir),
            session_key_rotation_secs: self
                .session_key_rotation_secs
                .or(base.session_key_rotation_secs),
            storage_key_file: self.storage_key_file.or(base.storage_key_file),
            kms_url: self.kms_url.or(base.kms_url),
            kms_identity: self.kms_identity.or(base.kms_identity),
            tee_release: self.tee_release.or(base.tee_release),
            tee_profile: self.tee_profile.or(base.tee_profile),
            peer_profiles: list(self.peer_profiles, base.peer_profiles),
            peer_min_release: self.peer_min_release.or(base.peer_min_release),
            registry_url: self.registry_url.or(base.registry_url),
            allow_unsigned_bundles: self.allow_unsigned_bundles.or(base.allow_unsigned_bundles),
            release_approvals: self.release_approvals.or(base.release_approvals),
            app_upgrades: self.app_upgrades.or(base.app_upgrades),
            outbound_approvals: self.outbound_approvals.or(base.outbound_approvals),
            apps_domain: self.apps_domain.or(base.apps_domain),
            settings: self.settings.or(base.settings),
            outbound: self.outbound.or(base.outbound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_given_flag_overrides_the_file_and_the_rest_is_kept() {
        let file = NodeArgs {
            node_id: Some(1),
            public_url: Some("http://a:2528".into()),
            peers: vec!["1=http://a:2528".into()],
            dev: Some(true),
            ..NodeArgs::default()
        };
        let given = NodeArgs {
            public_url: Some("http://b:2528".into()),
            dev: Some(false),
            ..NodeArgs::default()
        };
        let merged = given.over(file);
        assert_eq!(merged.node_id, Some(1));
        assert_eq!(merged.public_url.as_deref(), Some("http://b:2528"));
        assert_eq!(merged.peers, vec!["1=http://a:2528".to_owned()]);
        assert_eq!(
            merged.dev,
            Some(false),
            "`--dev false` turns the file's off"
        );
    }

    #[test]
    fn the_file_round_trips_with_its_outbound_table() {
        let text = r#"
node-id = 2
public-url = "http://n2:2528"
app-upgrades = "operator"

[outbound]
"#;
        let parsed: NodeArgs = toml::from_str(text).unwrap();
        assert_eq!(parsed.node_id, Some(2));
        assert_eq!(parsed.app_upgrades, Some(Authority::Operator));
        assert!(parsed.outbound.is_some());
        let again: NodeArgs = toml::from_str(&toml::to_string_pretty(&parsed).unwrap()).unwrap();
        assert_eq!(again.app_upgrades, Some(Authority::Operator));
        assert!(
            toml::from_str::<NodeArgs>("nodeid = 1").is_err(),
            "a typo is refused"
        );
    }
}
