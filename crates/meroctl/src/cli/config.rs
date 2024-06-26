//use std::fs;
use std::net::IpAddr;

//use std::path::Path;
use calimero_network::config::BootstrapNodes;
use clap::{Parser, ValueEnum};
use multiaddr::{Multiaddr, Protocol};
//use toml_edit::{DocumentMut, Item, Table, Value};
use tracing::info;

use crate::cli;
use crate::config_file::ConfigFile;

/// Initialize node configuration
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// List of bootstrap nodes
    #[arg(long, value_name = "ADDR")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[arg(long, value_name = "NETWORK", default_value = "calimero-dev")]
    pub boot_network: Option<BootstrapNetwork>,

    /// Host to listen on
    #[arg(long, value_name = "HOST", use_value_delimiter = true)]
    pub swarm_host: Vec<IpAddr>,

    /// Port to listen on
    #[arg(long, value_name = "PORT")]
    pub swarm_port: Option<u16>,

    /// Host to listen on for RPC
    #[arg(long, value_name = "HOST", use_value_delimiter = true)]
    pub server_host: Vec<IpAddr>,

    /// Port to listen on for RPC
    #[arg(long, value_name = "PORT")]
    pub server_port: Option<u16>,

    /// Enable mDNS discovery
    #[arg(long, default_value_t = true, overrides_with("no_mdns"))]
    pub mdns: bool,

    #[arg(long, hide = true, overrides_with("mdns"))]
    pub no_mdns: bool,

    /// Print the config file
    #[arg(long, short)]
    pub print: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNetwork {
    CalimeroDev,
    Ipfs,
}

impl ConfigCommand {
    pub fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&root_args.node_name);

        let mut config = if ConfigFile::exists(&path) {
            ConfigFile::load(&path)?
        } else {
            eyre::bail!("You have to initialize the node first \nRun command node init -n <NAME>");
        };

        if self.print {
            println!("{}", toml::to_string_pretty(&config)?);
            return Ok(());
        }

        // Update boot nodes if provided
        if !self.boot_nodes.is_empty() {
            config.network.bootstrap.nodes.list = self.boot_nodes;
        } else if let Some(network) = self.boot_network {
            config.network.bootstrap.nodes.list = match network {
                BootstrapNetwork::CalimeroDev => BootstrapNodes::calimero_dev().list,
                BootstrapNetwork::Ipfs => BootstrapNodes::ipfs().list,
            };
        }

        // Update swarm host and/or port if provided
        if !self.swarm_host.is_empty() || self.swarm_port.is_some() {
            let mut new_listen = Vec::new();

            let ipv4_host = self.swarm_host.iter().find_map(|ip| match ip {
                IpAddr::V4(v4) => Some(*v4),
                _ => None,
            });

            let ipv6_host = self.swarm_host.iter().find_map(|ip| match ip {
                IpAddr::V6(v6) => Some(*v6),
                _ => None,
            });
            for addr in config.network.swarm.listen.iter() {
                let mut new_addr = Multiaddr::empty();
                for protocol in addr.iter() {
                    match protocol {
                        Protocol::Ip4(_) if ipv4_host.is_some() => {
                            new_addr.push(Protocol::Ip4(ipv4_host.unwrap()));
                        }
                        Protocol::Ip6(_) if ipv6_host.is_some() => {
                            new_addr.push(Protocol::Ip6(ipv6_host.unwrap()));
                        }
                        Protocol::Tcp(_) | Protocol::Udp(_) if self.swarm_port.is_some() => {
                            let new_port = self.swarm_port.unwrap();
                            new_addr.push(match protocol {
                                Protocol::Tcp(_) => Protocol::Tcp(new_port),
                                Protocol::Udp(_) => Protocol::Udp(new_port),
                                _ => unreachable!(),
                            });
                        }
                        _ => new_addr.push(protocol),
                    }
                }
                new_listen.push(new_addr);
            }

            config.network.swarm.listen = new_listen;
        }

        // Update server host and/or port if provided
        if !self.server_host.is_empty() || self.server_port.is_some() {
            let mut new_listen = Vec::new();

            let ipv4_host = self.swarm_host.iter().find_map(|ip| match ip {
                IpAddr::V4(v4) => Some(*v4),
                _ => None,
            });

            let ipv6_host = self.swarm_host.iter().find_map(|ip| match ip {
                IpAddr::V6(v6) => Some(*v6),
                _ => None,
            });

            for addr in config.network.server.listen.iter() {
                let mut new_addr = Multiaddr::empty();
                for protocol in addr.iter() {
                    match protocol {
                        Protocol::Ip4(_) if ipv4_host.is_some() => {
                            new_addr.push(Protocol::Ip4(ipv4_host.unwrap()));
                        }
                        Protocol::Ip6(_) if ipv6_host.is_some() => {
                            new_addr.push(Protocol::Ip6(ipv6_host.unwrap()));
                        }
                        Protocol::Tcp(_) if self.server_port.is_some() => {
                            new_addr.push(Protocol::Tcp(self.server_port.unwrap()));
                        }
                        _ => new_addr.push(protocol),
                    }
                }
                new_listen.push(new_addr);
            }

            config.network.server.listen = new_listen;
        }

        // Update mDNS setting
        if self.mdns != self.no_mdns {
            config.network.discovery.mdns = self.mdns && !self.no_mdns;
        }

        config.save(&path)?;

        info!("Node configuration has been updated");

        Ok(())
    }
}
