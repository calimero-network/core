use std::fs;
use std::net::IpAddr;

use calimero_network::config::{BootstrapConfig, BootstrapNodes, DiscoveryConfig, SwarmConfig};
use calimero_node::config::{self, ConfigFile, NetworkConfig, StoreConfig};
use clap::{Parser, ValueEnum};
use eyre::WrapErr;
use libp2p::identity;
use multiaddr::Multiaddr;
use tracing::{info, warn};

use crate::cli;

#[derive(Debug, Parser)]
/// Initialize node configuration
// todo! simplify this, by splitting the steps
// todo! $ calimero node init
// todo! $ calimero node config 'swarm.listen:=["", ""]' server.graphql:=true discovery.mdns:=false
// todo! $ calimero node config discovery.mdns
pub struct InitCommand {
    /// List of bootstrap nodes
    #[clap(long, value_name = "ADDR")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[clap(long, value_name = "NETWORK")]
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
    Ipfs,
}

impl InitCommand {
    pub fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let mdns = self.mdns && !self.no_mdns;

        if !root_args.home.exists() {
            if root_args.home == config::default_chat_dir() {
                fs::create_dir_all(&root_args.home)
            } else {
                fs::create_dir(&root_args.home)
            }
            .wrap_err_with(|| format!("failed to create directory {:?}", root_args.home))?;
        }

        if ConfigFile::exists(&root_args.home) {
            if let Err(err) = ConfigFile::load(&root_args.home) {
                if self.force {
                    warn!(
                        "Failed to load existing configuration, overwriting: {}",
                        err
                    );
                } else {
                    eyre::bail!("failed to load existing configuration: {}", err);
                }
            }
            if !self.force {
                eyre::bail!("chat node is already initialized in {:?}", root_args.home);
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
        if let Some(BootstrapNetwork::Ipfs) = self.boot_network {
            boot_nodes.extend(BootstrapNodes::ipfs().list);
        }

        let config = ConfigFile {
            identity: identity.clone(),
            store: StoreConfig {
                path: "data".into(),
            },
            network: NetworkConfig {
                swarm: SwarmConfig { listen },
                bootstrap: BootstrapConfig {
                    nodes: BootstrapNodes { list: boot_nodes },
                },
                discovery: DiscoveryConfig { mdns },
                server: calimero_node::config::ServerConfig {
                    listen: self
                        .server_host
                        .into_iter()
                        .map(|host| {
                            Multiaddr::from(host).with(multiaddr::Protocol::Tcp(self.server_port))
                        })
                        .collect(),
                    admin: Some(calimero_server::admin::AdminConfig { enabled: true }),
                    graphql: Some(calimero_server::graphql::GraphQLConfig { enabled: true }),
                    websocket: Some(calimero_server::websocket::WsConfig { enabled: true }),
                },
            },
        };

        config.save(&root_args.home)?;

        calimero_store::Store::open(&calimero_store::config::StoreConfig {
            path: root_args.home.join(config.store.path),
        })?;

        info!("Initialized a chat node in {:?}", root_args.home);

        Ok(())
    }
}
