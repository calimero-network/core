use std::fs::{create_dir, create_dir_all};
use std::net::IpAddr;
use std::time::Duration;

use calimero_network::config::{
    BootstrapConfig, BootstrapNodes, CatchupConfig, DiscoveryConfig, RendezvousConfig, SwarmConfig,
};
use calimero_server::admin::service::AdminConfig;
use calimero_server::jsonrpc::JsonRpcConfig;
use calimero_server::ws::WsConfig;
use calimero_store::config::StoreConfig;
use calimero_store::db::RocksDB;
use calimero_store::Store;
use clap::{Parser, ValueEnum};
use eyre::{bail, Result as EyreResult, WrapErr};
use libp2p::identity::Keypair;
use multiaddr::{Multiaddr, Protocol};
use rand::{thread_rng, Rng};
use tracing::{info, warn};

use crate::config_file::{
    ApplicationConfig, ConfigFile, NetworkConfig, ServerConfig, StoreConfig as StoreConfigFile,
};
use crate::{cli, defaults};

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
    #[clap(default_value_t = calimero_network::config::DEFAULT_PORT)]
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

    /// Enable mDNS discovery
    #[clap(long, default_value_t = true)]
    #[clap(overrides_with("no_mdns"))]
    pub mdns: bool,

    #[clap(long, hide = true)]
    #[clap(overrides_with("mdns"))]
    pub no_mdns: bool,

    /// Force initialization even if the directory already exists
    #[clap(long)]
    pub force: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNetwork {
    CalimeroDev,
    Ipfs,
}

impl InitCommand {
    // TODO: Consider splitting this function up to reduce complexity.
    #[allow(clippy::cognitive_complexity)]
    pub fn run(self, root_args: cli::RootArgs) -> EyreResult<()> {
        let mdns = self.mdns && !self.no_mdns;

        let path = root_args.home.join(root_args.node_name);

        if !path.exists() {
            if root_args.home == defaults::default_node_dir() {
                create_dir_all(&path)
            } else {
                create_dir(&path)
            }
            .wrap_err_with(|| format!("failed to create directory {path:?}"))?;
        }

        if ConfigFile::exists(&path) {
            if let Err(err) = ConfigFile::load(&path) {
                if self.force {
                    warn!(
                        "Failed to load existing configuration, overwriting: {}",
                        err
                    );
                } else {
                    bail!("Failed to load existing configuration: {}", err);
                }
            }
            if !self.force {
                bail!("Node is already initialized in {:?}", path);
            }
        }

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

        let config = ConfigFile {
            identity,
            store: StoreConfigFile {
                path: "data".into(),
            },
            application: ApplicationConfig {
                path: "apps".into(),
            },
            network: NetworkConfig {
                swarm: SwarmConfig::new(listen),
                bootstrap: BootstrapConfig::new(BootstrapNodes::new(boot_nodes)),
                discovery: DiscoveryConfig::new(mdns, RendezvousConfig::default()),
                server: ServerConfig {
                    listen: self
                        .server_host
                        .into_iter()
                        .map(|host| Multiaddr::from(host).with(Protocol::Tcp(self.server_port)))
                        .collect(),
                    admin: Some(AdminConfig::new(true)),
                    jsonrpc: Some(JsonRpcConfig::new(true)),
                    websocket: Some(WsConfig::new(true)),
                },
                catchup: CatchupConfig::new(
                    50,
                    Duration::from_secs(2),
                    Duration::from_secs(2),
                    Duration::from_millis(thread_rng().gen_range(0..1001)),
                ),
            },
        };

        config.save(&path)?;

        drop(Store::open::<RocksDB>(&StoreConfig::new(
            path.join(config.store.path),
        ))?);

        info!("Initialized a node in {:?}", path);

        Ok(())
    }
}
