use calimero_config::{
    BlobStoreConfig, ConfigFile, DataStoreConfig as StoreConfigFile, IdentityConfig, NetworkConfig,
    NodeMode, ServerConfig, SyncConfig, CONFIG_FILE,
};
use calimero_context::config::ContextConfig;
use calimero_context_config::client_config::{ClientConfig, ClientSigner, LocalConfig};
use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, RelayConfig, RendezvousConfig,
    SwarmConfig,
};
use calimero_server::admin::service::AdminConfig;
use calimero_server::config::AuthMode;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::sse::SseConfig;
use calimero_server::ws::WsConfig;
use calimero_store::config::StoreConfig;
use calimero_store::Store;
use calimero_store_rocksdb::RocksDB;
use clap::{Parser, ValueEnum};
use core::net::IpAddr;
use core::time::Duration;
use eyre::{bail, Result as EyreResult, WrapErr};
use libp2p::identity::Keypair;
use mero_auth::config::{AuthConfig as EmbeddedAuthConfig, StorageConfig as AuthStorageConfig};
use multiaddr::{Multiaddr, Protocol};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{info, warn};

use super::auth_mode::AuthModeArg;
use crate::cli;

/// Restrict a single path to the given owner-only `mode` (`0700` for
/// directories, `0600` for files). No-op on non-Unix platforms, which lack
/// POSIX mode bits.
#[cfg(unix)]
async fn restrict_to_owner(path: impl AsRef<Path>, mode: u32) -> EyreResult<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = path.as_ref();
    fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .await
        .wrap_err_with(|| format!("failed to restrict permissions on {path:?}"))
}

#[cfg(not(unix))]
async fn restrict_to_owner(_path: impl AsRef<Path>, _mode: u32) -> EyreResult<()> {
    Ok(())
}

/// Recursively restrict a directory tree to owner-only access: `0700` for every
/// directory and `0600` for every file. RocksDB creates the datastore's files
/// and sub-directories with the process umask (typically world-readable), so
/// walking the tree after the store is closed makes the raw data — and any
/// content left by a previous partial init — unreadable to other local users
/// rather than relying solely on the top-level directory's mode. Symlinks are
/// left untouched (RocksDB creates none here).
#[cfg(unix)]
async fn restrict_tree_to_owner(root: impl AsRef<Path>) -> EyreResult<()> {
    let mut stack = vec![root.as_ref().to_path_buf()];

    while let Some(dir) = stack.pop() {
        restrict_to_owner(&dir, 0o700).await?;

        let mut entries = fs::read_dir(&dir)
            .await
            .wrap_err_with(|| format!("failed to read directory {dir:?}"))?;

        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else if file_type.is_file() {
                restrict_to_owner(entry.path(), 0o600).await?;
            }
        }
    }

    Ok(())
}

#[cfg(not(unix))]
async fn restrict_tree_to_owner(_root: impl AsRef<Path>) -> EyreResult<()> {
    Ok(())
}

/// Create `path` and any missing parents, with directories created owner-only
/// (`0700`) on Unix. Using the mode at creation time means the node home is
/// never momentarily visible to other users with permissive bits — the window a
/// create-then-`chmod` would leave open. Pre-existing components keep their mode.
async fn create_dir_owner_only(path: impl AsRef<Path>) -> EyreResult<()> {
    let path = path.as_ref();

    let mut builder = fs::DirBuilder::new();
    let _ = builder.recursive(true);
    #[cfg(unix)]
    let _ = builder.mode(0o700);

    builder
        .create(path)
        .await
        .wrap_err_with(|| format!("failed to create directory {path:?}"))
}

// Sync configuration - aggressive defaults for fast CRDT convergence
const DEFAULT_SYNC_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SYNC_SESSION_DEADLINE: Duration = Duration::from_secs(30);
const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_SYNC_FREQUENCY: Duration = Duration::from_secs(10);

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum AuthStorageArg {
    Persistent,
    Memory,
}

/// Initialize node configuration
#[derive(Debug, Parser)]
pub struct InitCommand {
    /// List of bootstrap nodes
    #[clap(long, value_name = "ADDR")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[clap(long, value_name = "NETWORK", default_value = "calimero-dev")]
    pub boot_network: Option<BootstrapNetwork>,

    /// Host to listen on
    #[clap(long, value_name = "HOST")]
    #[clap(default_value = "0.0.0.0,::")]
    #[clap(use_value_delimiter = true)]
    pub swarm_host: Vec<IpAddr>,

    /// Port to listen on
    #[clap(long, value_name = "PORT")]
    #[clap(default_value_t = calimero_network_primitives::config::DEFAULT_PORT)]
    pub swarm_port: u16,

    /// Host to listen on for RPC
    #[clap(long, value_name = "HOST")]
    #[clap(default_value = "127.0.0.1,::1")]
    #[clap(use_value_delimiter = true)]
    pub server_host: Vec<IpAddr>,

    /// Port to listen on for RPC
    #[clap(long, value_name = "PORT")]
    #[clap(default_value_t = calimero_server::config::DEFAULT_PORT)]
    pub server_port: u16,

    /// Authentication mode for server endpoints
    #[clap(long, value_enum)]
    pub auth_mode: Option<AuthModeArg>,

    /// Embedded auth storage implementation (only used when auth mode is embedded)
    #[clap(long, value_enum)]
    pub auth_storage: Option<AuthStorageArg>,

    /// Embedded auth storage path (only used with persistent storage)
    #[clap(long, value_name = "PATH")]
    pub auth_storage_path: Option<PathBuf>,

    /// Enable mDNS discovery
    #[clap(long, default_value_t = true)]
    #[clap(overrides_with("no_mdns"))]
    pub mdns: bool,

    #[clap(
        long,
        hide = true,
        help = "Disable mDNS discovery (hidden as it's the inverse of --mdns)"
    )]
    #[clap(overrides_with("mdns"))]
    pub no_mdns: bool,

    /// Advertise observed address
    #[clap(long, default_value_t = false)]
    #[clap(overrides_with("no_mdns"))]
    pub advertise_address: bool,

    /// Static external multiaddr(s) to advertise (e.g.
    /// `/ip4/203.0.113.7/tcp/2428`). Seeded directly into the swarm's
    /// external-address set at startup; requires `--advertise-address`.
    /// AutoNAT v2 always additionally discovers and confirms reachable
    /// addresses regardless of this flag. Non-routable values (loopback /
    /// unspecified / link-local) are ignored.
    #[clap(long = "external-address", value_name = "MULTIADDR")]
    pub external_address: Vec<Multiaddr>,

    #[clap(
        long,
        default_value = "3",
        help = "Maximum number of rendezvous registrations allowed"
    )]
    pub rendezvous_registrations_limit: usize,

    #[clap(
        long,
        default_value = "3",
        help = "Maximum number of relay registrations allowed"
    )]
    pub relay_registrations_limit: usize,

    #[clap(
        long,
        default_value_t = 10,
        help = "The interval between AutoNAT probes. Default is 10 seconds"
    )]
    pub autonat_probe_interval: u64,

    #[clap(
        long,
        default_value = "5",
        help = "Maximum number of untested addresses candidates to test with AutoNAT probes"
    )]
    pub autonat_max_candidates: usize,

    /// Force initialization even if the directory already exists
    #[clap(long)]
    pub force: bool,

    /// Node operation mode (standard or read-only)
    #[clap(long, value_enum, default_value_t = NodeMode::Standard)]
    pub mode: NodeMode,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNetwork {
    CalimeroDev,
    Ipfs,
}

impl InitCommand {
    // TODO: Consider splitting this function up to reduce complexity.
    #[expect(
        clippy::cognitive_complexity,
        clippy::too_many_lines,
        reason = "TODO: Will be refactored"
    )]
    pub async fn run(self, root_args: cli::RootArgs) -> EyreResult<()> {
        let mdns = self.mdns && !self.no_mdns;

        let path = root_args.home.join(root_args.node_name);

        if ConfigFile::exists(&path) {
            if let Err(err) = ConfigFile::load(&path).await {
                if self.force {
                    warn!(
                        "Failed to load existing configuration, overwriting: {}",
                        err
                    );
                } else {
                    bail!("Failed to load existing configuration: {}", err);
                }
            } else if !self.force {
                warn!("Node is already initialized in {:?}", path);
                return Ok(());
            }

            fs::remove_dir_all(&path).await?;
        }

        if !path.exists() {
            create_dir_owner_only(&path).await?;
        }

        // A freshly created home is already 0700 (above); tighten a pre-existing
        // one so the private key and datastore land in an owner-only directory.
        restrict_to_owner(&path, 0o700).await?;

        let identity = Keypair::generate_ed25519();
        info!("Generated identity: {:?}", identity.public().to_peer_id());

        let mut listen: Vec<Multiaddr> = vec![];

        for host in self.swarm_host {
            let host = format!(
                "/{}/{}",
                match host {
                    IpAddr::V4(_) => "ip4",
                    IpAddr::V6(_) => "ip6",
                },
                host,
            );
            listen.push(format!("{}/tcp/{}", host, self.swarm_port).parse()?);
            listen.push(format!("{}/udp/{}/quic-v1", host, self.swarm_port).parse()?);
        }

        let mut boot_nodes = self.boot_nodes;
        if let Some(network) = self.boot_network {
            match network {
                BootstrapNetwork::CalimeroDev => {
                    boot_nodes.extend(BootstrapNodes::calimero_dev().list);
                }
                BootstrapNetwork::Ipfs => boot_nodes.extend(BootstrapNodes::ipfs().list),
            }
        }

        let client_config = ClientConfig {
            signer: ClientSigner {
                local: LocalConfig {
                    protocols: BTreeMap::new(),
                },
            },
        };

        let auth_mode = self.auth_mode.map(Into::into).unwrap_or(AuthMode::Proxy);
        let embedded_auth = if matches!(auth_mode, AuthMode::Embedded) {
            let mut auth_cfg: EmbeddedAuthConfig = mero_auth::embedded::default_config();
            let storage_choice = self.auth_storage.unwrap_or(AuthStorageArg::Persistent);
            let storage_path = self.auth_storage_path.clone();

            match storage_choice {
                AuthStorageArg::Persistent => {
                    let path = storage_path.unwrap_or_else(|| PathBuf::from("auth"));
                    auth_cfg.storage = AuthStorageConfig::RocksDB { path };
                }
                AuthStorageArg::Memory => {
                    if let Some(path) = storage_path {
                        warn!(
                            "Ignoring --auth-storage-path={} because in-memory storage is selected",
                            path.display()
                        );
                    }
                    auth_cfg.storage = AuthStorageConfig::Memory;
                }
            }

            Some(auth_cfg)
        } else {
            None
        };

        let server_config = ServerConfig::with_auth(
            self.server_host
                .into_iter()
                .map(|host| Multiaddr::from(host).with(Protocol::Tcp(self.server_port)))
                .collect(),
            Some(AdminConfig::new(true)),
            Some(JsonRpcConfig::new(true)),
            Some(WsConfig::new(true)),
            Some(SseConfig::new(true)),
            auth_mode,
            embedded_auth,
        );

        let config = ConfigFile::new(
            IdentityConfig { keypair: identity },
            self.mode,
            NetworkConfig::new(
                SwarmConfig::new(listen),
                BootstrapConfig::new(BootstrapNodes::new(boot_nodes)),
                DiscoveryConfig::new(
                    mdns,
                    self.advertise_address,
                    self.external_address.clone(),
                    RendezvousConfig::new(self.rendezvous_registrations_limit),
                    RelayConfig::new(self.relay_registrations_limit),
                    AutonatConfig::new(
                        self.autonat_max_candidates,
                        Duration::from_secs(self.autonat_probe_interval),
                    ),
                ),
                server_config,
            ),
            SyncConfig::new(
                DEFAULT_SYNC_TIMEOUT,
                DEFAULT_SYNC_SESSION_DEADLINE,
                DEFAULT_SYNC_INTERVAL,
                DEFAULT_SYNC_FREQUENCY,
            ),
            StoreConfigFile::new("data".into()),
            BlobStoreConfig::new("blobs".into()),
            ContextConfig {
                client: client_config,
                migration_v2: true,
            },
        );

        config.save(&path).await?;

        // The file itself holds the private key; keep it owner-only even if its
        // parent directory's permissions are ever loosened.
        restrict_to_owner(path.join(CONFIG_FILE), 0o600).await?;

        // `config` is fully consumed below; `datastore_path` is cloned so the
        // store's owned copy is independent.
        let datastore_path = path.join(&config.datastore.path);
        drop(Store::open::<RocksDB>(&StoreConfig::new(
            datastore_path.clone(),
        ))?);

        // RocksDB creates these files under the process umask, but they live
        // inside the now-0700 node home, so they were never reachable by other
        // users. With the store closed (no writer racing us), recursively pin the
        // datastore and every RocksDB file to owner-only as defense in depth, so
        // the contents stay private even if the home's mode is later loosened.
        restrict_tree_to_owner(&datastore_path).await?;

        info!("Initialized a node in {:?}", path);

        Ok(())
    }
}
