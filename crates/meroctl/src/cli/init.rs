use std::fs;
use std::net::IpAddr;

use calimero_network::config::{BootstrapConfig, BootstrapNodes, DiscoveryConfig, SwarmConfig};
use clap::{Parser, ValueEnum};
use eyre::WrapErr;
use libp2p::identity;
use multiaddr::Multiaddr;
use tracing::{info, warn};

use crate::config_file::{ApplicationConfig, ConfigFile, NetworkConfig, ServerConfig, StoreConfig};
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
    pub fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let mdns = self.mdns && !self.no_mdns;

        let path = root_args.home.join(root_args.node_name);

        if !path.exists() {
            if root_args.home == defaults::default_node_dir() {
                fs::create_dir_all(&path)
            } else {
                fs::create_dir(&path)
            }
            .wrap_err_with(|| format!("failed to create directory {:?}", path))?;
        }

        if ConfigFile::exists(&path) {
            if let Err(err) = ConfigFile::load(&path) {
                if self.force {
                    warn!(
                        "Failed to load existing configuration, overwriting: {}",
                        err
                    );
                } else {
                    eyre::bail!("Failed to load existing configuration: {}", err);
                }
            }
            if !self.force {
                eyre::bail!("Node is already initialized in {:?}", path);
            }
        }

        let identity = identity::Keypair::generate_ed25519();
        info!("Generated identity: {:?}", identity.public().to_peer_id());

        let mut listen: Vec<Multiaddr> = vec![];

        for host in self.swarm_host {
            let host = format!(
                "/{}/{}",
                match host {
                    std::net::IpAddr::V4(_) => "ip4",
                    std::net::IpAddr::V6(_) => "ip6",
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
                    boot_nodes.extend(BootstrapNodes::calimero_dev().list)
                }
                BootstrapNetwork::Ipfs => boot_nodes.extend(BootstrapNodes::ipfs().list),
            }
        }

        let config = ConfigFile {
            identity: identity.clone(),
            store: StoreConfig {
                path: "data".into(),
            },
            application: ApplicationConfig {
                path: "apps".into(),
            },
            network: NetworkConfig {
                swarm: SwarmConfig { listen },
                bootstrap: BootstrapConfig {
                    nodes: BootstrapNodes { list: boot_nodes },
                },
                discovery: DiscoveryConfig {
                    mdns,
                    rendezvous: Default::default(),
                },
                server: ServerConfig {
                    listen: self
                        .server_host
                        .into_iter()
                        .map(|host| {
                            Multiaddr::from(host).with(multiaddr::Protocol::Tcp(self.server_port))
                        })
                        .collect(),
                    admin: Some(calimero_server::admin::service::AdminConfig { enabled: true }),
                    jsonrpc: Some(calimero_server::jsonrpc::JsonRpcConfig { enabled: true }),
                    websocket: Some(calimero_server::ws::WsConfig { enabled: true }),
                },
            },
        };

        config.save(&path)?;

        calimero_store::Store::open(&calimero_store::config::StoreConfig {
            path: path.join(config.store.path),
        })?;

        info!("Initialized a node in {:?}", path);

        Ok(())
    }
}
