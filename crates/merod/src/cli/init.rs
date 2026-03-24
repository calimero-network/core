use calimero_config::{
    BlobStoreConfig, ConfigFile, DataStoreConfig as StoreConfigFile, GroupIdentityConfig,
    IdentityConfig, NetworkConfig, NodeMode, ServerConfig, SyncConfig,
};
use calimero_context::config::{ContextConfig, GroupGovernanceMode};
use calimero_context_config::client::config::{
    ClientConfig, ClientRelayerSigner, ClientSigner, LocalConfig,
};
#[cfg(feature = "near_init")]
use calimero_context_config::client::config::{
    ClientConfigParams, ClientLocalConfig, ClientLocalSigner, ClientSelectedSigner, Credentials,
};
#[cfg(feature = "near_init")]
use calimero_context_config::client::protocol::near as near_protocol;
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
#[cfg(feature = "near_init")]
use hex::encode;
use libp2p::identity::Keypair;
use mero_auth::config::{AuthConfig as EmbeddedAuthConfig, StorageConfig as AuthStorageConfig};
use multiaddr::{Multiaddr, Protocol};
#[cfg(feature = "near_init")]
use near_crypto::{KeyType, SecretKey};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::fs::{self, create_dir_all};
use tracing::{info, warn};
use url::Url;

use super::auth_mode::AuthModeArg;
use crate::{cli, defaults};

// Sync configuration - aggressive defaults for fast CRDT convergence
const DEFAULT_SYNC_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_SYNC_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_SYNC_FREQUENCY: Duration = Duration::from_secs(10);

/// Helper struct to define protocol configuration
#[cfg(feature = "near_init")]
#[derive(Debug)]
struct ProtocolConfig<'a> {
    name: &'a str,
    default_network: &'a str,
    default_contract: &'a str,
    signer_type: ClientSelectedSigner,
    networks: &'a [(&'a str, &'a str)],
    protocol: ConfigProtocol,
}

/// Protocol configurations for all supported protocols
#[cfg(feature = "near_init")]
const PROTOCOL_CONFIGS: &[ProtocolConfig<'static>] = &[ProtocolConfig {
    name: "near",
    default_network: "testnet",
    default_contract: "v0-7.config.calimero-context.testnet",
    signer_type: ClientSelectedSigner::Relayer,
    networks: &[
        ("mainnet", "https://rpc.mainnet.near.org"),
        ("testnet", "https://rpc.testnet.near.org"),
    ],
    protocol: ConfigProtocol::Near,
}];

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
pub enum ConfigProtocol {
    Near,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum GroupGovernanceInitArg {
    #[default]
    External,
    Local,
}

impl From<GroupGovernanceInitArg> for GroupGovernanceMode {
    fn from(value: GroupGovernanceInitArg) -> Self {
        match value {
            GroupGovernanceInitArg::External => GroupGovernanceMode::External,
            GroupGovernanceInitArg::Local => GroupGovernanceMode::Local,
        }
    }
}

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

    /// URL of the relayer for submitting NEAR transactions
    #[clap(long, value_name = "URL")]
    pub relayer_url: Option<Url>,

    /// Name of protocol
    #[clap(long, value_name = "PROTOCOL", default_value = "near")]
    #[clap(value_enum)]
    pub protocol: ConfigProtocol,

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
    /// Node operation mode (standard or read-only)
    #[clap(long, value_enum, default_value_t = NodeMode::Standard)]
    pub mode: NodeMode,

    /// Group policy: `external` (chain + relayer) or `local` (signed gossip only). For `local`, NEAR
    /// protocol blocks are omitted from the generated context client config (add them later if you
    /// need chain-backed contexts or `join_group_context` bootstrap against NEAR).
    #[clap(long, value_enum, default_value_t = GroupGovernanceInitArg::External)]
    pub group_governance: GroupGovernanceInitArg,
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
            create_dir_all(&path)
                .await
                .wrap_err_with(|| format!("failed to create directory {path:?}"))?;
        }

        let identity = Keypair::generate_ed25519();
        info!("Generated identity: {:?}", identity.public().to_peer_id());

        let group_sk = ed25519_consensus::SigningKey::new(rand::thread_rng());
        let group_vk = group_sk.verification_key();
        let group_identity = GroupIdentityConfig {
            public_key: format!(
                "ed25519:{}",
                bs58::encode(group_vk.as_bytes()).into_string()
            ),
            secret_key: format!(
                "ed25519:{}",
                bs58::encode(group_sk.as_bytes()).into_string()
            ),
        };
        info!("Generated group identity: {}", group_identity.public_key);

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

        #[cfg(feature = "near_init")]
        let mut local_signers = LocalConfig {
            protocols: BTreeMap::default(),
        };
        #[cfg(not(feature = "near_init"))]
        let local_signers = LocalConfig {
            protocols: BTreeMap::default(),
        };

        #[cfg(feature = "near_init")]
        let mut client_params = BTreeMap::default();
        #[cfg(not(feature = "near_init"))]
        let client_params = BTreeMap::default();

        let group_governance_mode: GroupGovernanceMode = self.group_governance.into();

        #[cfg(not(feature = "near_init"))]
        if group_governance_mode != GroupGovernanceMode::Local {
            bail!(
                "this merod binary was built without NEAR init support (`near_init` Cargo feature disabled); \
                 use `merod init --group-governance local` or rebuild with default features"
            );
        }

        // NEAR protocol blocks: only for external governance (or mixed deployments). Local-only
        // nodes omit them so `config.toml` does not require chain coordinates; see
        // `docs/context-management/LOCAL-GROUP-GOVERNANCE.md`.
        #[cfg(feature = "near_init")]
        if group_governance_mode != GroupGovernanceMode::Local {
            for config in PROTOCOL_CONFIGS {
                let _ignored = client_params.insert(
                    config.name.to_owned(),
                    ClientConfigParams {
                        network: config.default_network.into(),
                        contract_id: config.default_contract.parse()?,
                        signer: config.signer_type,
                    },
                );

                let mut local_config = ClientLocalConfig {
                    signers: Default::default(),
                };

                for (network_name, rpc_url) in config.networks {
                    let _ignored = local_config.signers.insert(
                        network_name.to_string(),
                        generate_local_signer(rpc_url.parse()?, config.protocol)?,
                    );
                }

                let _ignored = local_signers
                    .protocols
                    .insert(config.name.to_owned(), local_config);
            }
        }

        let relayer_signer = if group_governance_mode == GroupGovernanceMode::Local {
            None
        } else {
            Some(ClientRelayerSigner {
                url: self
                    .relayer_url
                    .unwrap_or_else(defaults::default_relayer_url),
            })
        };

        let client_config = ClientConfig {
            signer: ClientSigner {
                relayer: relayer_signer,
                local: local_signers,
            },
            params: client_params,
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
            IdentityConfig {
                keypair: identity,
                group: Some(group_identity),
            },
            self.mode,
            NetworkConfig::new(
                SwarmConfig::new(listen),
                BootstrapConfig::new(BootstrapNodes::new(boot_nodes)),
                DiscoveryConfig::new(
                    mdns,
                    self.advertise_address,
                    RendezvousConfig::new(self.rendezvous_registrations_limit),
                    RelayConfig::new(self.relay_registrations_limit),
                    AutonatConfig::new(
                        self.autonat_max_candidates,
                        Duration::from_secs(self.autonat_probe_interval),
                    ),
                ),
                server_config,
            ),
            SyncConfig {
                timeout: DEFAULT_SYNC_TIMEOUT,
                interval: DEFAULT_SYNC_INTERVAL,
                frequency: DEFAULT_SYNC_FREQUENCY,
            },
            StoreConfigFile::new("data".into()),
            BlobStoreConfig::new("blobs".into()),
            ContextConfig {
                client: client_config,
                group_governance: group_governance_mode,
            },
        );

        config.save(&path).await?;

        drop(Store::open::<RocksDB>(&StoreConfig::new(
            path.join(config.datastore.path),
        ))?);

        if group_governance_mode == GroupGovernanceMode::Local {
            info!(
                "Initialized a node in {:?} (group_governance=local; no NEAR protocol in context client config)",
                path
            );
        } else {
            info!("Initialized a node in {:?}", path);
        }

        Ok(())
    }
}

#[cfg(feature = "near_init")]
fn generate_local_signer(
    rpc_url: Url,
    config_protocol: ConfigProtocol,
) -> EyreResult<ClientLocalSigner> {
    match config_protocol {
        ConfigProtocol::Near => {
            let secret_key = SecretKey::from_random(KeyType::ED25519);
            let public_key = secret_key.public_key();
            let account_id = public_key.unwrap_as_ed25519().0;

            Ok(ClientLocalSigner {
                rpc_url,
                credentials: Credentials::Near(near_protocol::Credentials {
                    account_id: encode(account_id).parse()?,
                    public_key,
                    secret_key,
                }),
            })
        }
    }
}
