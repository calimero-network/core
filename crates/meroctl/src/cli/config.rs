use std::fs;
use std::net::IpAddr;

use calimero_network::config::BootstrapNodes;
use clap::{Parser, ValueEnum};
use multiaddr::{Multiaddr, Protocol};
use toml_edit::{DocumentMut, Value};
use tracing::info;

use crate::cli;

/// Initialize node configuration
#[derive(Debug, Parser)]
pub struct ConfigCommand {
    /// List of bootstrap nodes
    #[arg(long, value_name = "ADDR", conflicts_with = "boot_network")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[arg(long, value_name = "NETWORK", conflicts_with = "boot_nodes")]
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
        let path = root_args
            .home
            .join(&root_args.node_name)
            .join("config.toml");

        // Load the existing TOML file
        let toml_str = fs::read_to_string(&path)
            .map_err(|_| eyre::eyre!("Node must be initialized first."))?;
        let mut doc = toml_str.parse::<DocumentMut>()?;

        if self.print {
            println!("{}", doc.to_string());
            return Ok(());
        }

        let ipv4_host = self.swarm_host.iter().find_map(|ip| {
            if let IpAddr::V4(v4) = ip {
                Some(*v4)
            } else {
                None
            }
        });

        let ipv6_host = self.swarm_host.iter().find_map(|ip| {
            if let IpAddr::V6(v6) = ip {
                Some(*v6)
            } else {
                None
            }
        });

        // Update swarm listen addresses
        if !self.swarm_host.is_empty() || self.swarm_port.is_some() {
            let swarm_table = doc["swarm"].as_table_mut().unwrap();
            let listen_array = swarm_table["listen"].as_array_mut().unwrap();

            for item in listen_array.iter_mut() {
                let addr: Multiaddr = item.as_str().unwrap().parse().unwrap();
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

                *item = Value::from(new_addr.to_string());
            }
        }

        // Update server listen addresses
        if !self.server_host.is_empty() || self.server_port.is_some() {
            let server_table = doc["server"].as_table_mut().unwrap();
            let listen_array = server_table["listen"].as_array_mut().unwrap();

            for item in listen_array.iter_mut() {
                let addr: Multiaddr = item.as_str().unwrap().parse().unwrap();
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

                *item = Value::from(new_addr.to_string());
            }
        }

        // Update boot nodes if provided
        if !self.boot_nodes.is_empty() {
            let bootstrap_table = doc["bootstrap"].as_table_mut().unwrap();
            let list_array = bootstrap_table["nodes"].as_array_mut().unwrap();
            list_array.clear();
            for node in self.boot_nodes.iter() {
                list_array.push(node.to_string());
            }
        } else if let Some(network) = self.boot_network {
            let bootstrap_table = doc["bootstrap"].as_table_mut().unwrap();
            let list_array = bootstrap_table["nodes"].as_array_mut().unwrap();
            list_array.clear();
            let new_nodes = match network {
                BootstrapNetwork::CalimeroDev => BootstrapNodes::calimero_dev().list,
                BootstrapNetwork::Ipfs => BootstrapNodes::ipfs().list,
            };
            for node in new_nodes.iter() {
                list_array.push(node.to_string());
            }
        }

        // Update mDNS setting if provided
        if self.mdns != self.no_mdns {
            let discovery_table = doc["discovery"].as_table_mut().unwrap();
            discovery_table["mdns"] = toml_edit::value(self.mdns && !self.no_mdns);
        }

        // Save the updated TOML back to the file
        fs::write(&path, doc.to_string())?;

        info!("Node configuration has been updated");

        Ok(())
    }
}
