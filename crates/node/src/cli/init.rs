use std::fs;
use std::net::IpAddr;

use calimero_network::config::{
    AppConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, EndpointConfig, SwarmConfig,
};
use calimero_node::config::{self, ConfigFile, NetworkConfig, StoreConfig};
use clap::{Parser, ValueEnum};
use eyre::WrapErr;
use libp2p::{identity, Multiaddr};
use tracing::{info, warn};

use crate::cli;

#[derive(Debug, Parser)]
/// Initialize node configuration
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
    pub host: Vec<IpAddr>,

    /// Port to listen on
    #[clap(long, value_name = "PORT")]
    #[clap(default_value_t = calimero_network::config::DEFAULT_PORT)]
    pub port: u16,

    /// Host to listen on
    #[clap(long, value_name = "RPC_HOST")]
    #[clap(default_value = "127.0.0.1")]
    pub rpc_host: String,

    /// Port to listen on
    #[clap(long, value_name = "RPC_PORT")]
    #[clap(default_value_t = calimero_network::config::DEFAULT_RPC_PORT)]
    pub rpc_port: u16,

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

        for host in self.host {
            let host = format!(
                "/{}/{}",
                match host {
                    std::net::IpAddr::V4(_) => "ip4",
                    std::net::IpAddr::V6(_) => "ip6",
                },
                host,
            );
            listen.push(format!("{}/tcp/{}", host, self.port).parse()?);
            listen.push(format!("{}/udp/{}/quic-v1", host, self.port).parse()?);
        }

        let mut boot_nodes = self.boot_nodes;
        if let Some(BootstrapNetwork::Ipfs) = self.boot_network {
            boot_nodes.extend(BootstrapNodes::ipfs().list);
        }

        let config = ConfigFile {
            identity,
            store: StoreConfig {
                path: "data".into(),
            },
            network: NetworkConfig {
                swarm: SwarmConfig { listen },
                bootstrap: BootstrapConfig {
                    nodes: BootstrapNodes { list: boot_nodes },
                },
                discovery: DiscoveryConfig { mdns },
                endpoint: EndpointConfig {
                    host: self.rpc_host,
                    port: self.rpc_port,
                },
                app: AppConfig {
                    wasm_path: "".to_string(),
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
