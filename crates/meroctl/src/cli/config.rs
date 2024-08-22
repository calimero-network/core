#![allow(unused_results)]

use std::fs;
use std::net::IpAddr;

use calimero_network::config::BootstrapNodes;
use clap::{Args, Parser, ValueEnum};
use eyre::eyre;
use multiaddr::{Multiaddr, Protocol};
use toml_edit::{DocumentMut, Value};
use tracing::info;

use crate::cli;

/// Configure the node
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

    #[command(flatten)]
    pub mdns: Option<MdnsArgs>,

    /// Print the config file
    #[arg(long, short)]
    pub print: bool,
}

#[derive(Args, Debug)]
#[group(multiple = false)]
pub struct MdnsArgs {
    /// Enable mDNS discovery
    #[arg(long)]
    pub mdns: bool,

    #[arg(long, hide = true)]
    pub _no_mdns: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNetwork {
    CalimeroDev,
    Ipfs,
}

#[warn(unused_results)]
impl ConfigCommand {
    pub fn run(self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let path = root_args
            .home
            .join(&root_args.node_name)
            .join("config.toml");

        // Load the existing TOML file
        let toml_str = fs::read_to_string(&path)
            .map_err(|_| eyre!("Node is not initialized in {:?}", path))?;
        let mut doc = toml_str.parse::<DocumentMut>()?;

        if self.print {
            println!("{}", doc);
            return Ok(());
        }

        let (ipv4_host, ipv6_host) =
            self.swarm_host
                .iter()
                .fold((None, None), |(v4, v6), ip| match ip {
                    IpAddr::V4(v4_addr) => (Some(*v4_addr), v6),
                    IpAddr::V6(v6_addr) => (v4, Some(*v6_addr)),
                });

        // Update swarm listen addresses
        if !self.swarm_host.is_empty() || self.swarm_port.is_some() {
            let listen_array = doc["swarm"]["listen"]
                .as_array_mut()
                .ok_or(eyre!("No swarm table in config.toml"))?;

            for item in listen_array.iter_mut() {
                let addr: Multiaddr = item
                    .as_str()
                    .ok_or(eyre!("Value can't be parsed as string"))?
                    .parse()?;
                let mut new_addr = Multiaddr::empty();

                for protocol in addr.iter() {
                    match (&protocol, ipv4_host, ipv6_host, self.swarm_port) {
                        (Protocol::Ip4(_), Some(ipv4_host), _, _) => {
                            new_addr.push(Protocol::Ip4(ipv4_host));
                        }
                        (Protocol::Ip6(_), _, Some(ipv6_host), _) => {
                            new_addr.push(Protocol::Ip6(ipv6_host));
                        }
                        (Protocol::Tcp(_) | Protocol::Udp(_), _, _, Some(new_port)) => {
                            new_addr.push(match &protocol {
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
            let listen_array = doc["server"]["listen"]
                .as_array_mut()
                .ok_or(eyre!("No server table in config.toml"))?;

            for item in listen_array.iter_mut() {
                let addr: Multiaddr = item
                    .as_str()
                    .ok_or(eyre!("Value can't be parsed as string"))?
                    .parse()?;
                let mut new_addr = Multiaddr::empty();

                for protocol in addr.iter() {
                    match (&protocol, ipv4_host, ipv6_host, self.server_port) {
                        (Protocol::Ip4(_), Some(ipv4_host), _, _) => {
                            new_addr.push(Protocol::Ip4(ipv4_host));
                        }
                        (Protocol::Ip6(_), _, Some(ipv6_host), _) => {
                            new_addr.push(Protocol::Ip6(ipv6_host));
                        }
                        (Protocol::Tcp(_), _, _, Some(new_port)) => {
                            new_addr.push(Protocol::Tcp(new_port));
                        }
                        _ => new_addr.push(protocol),
                    }
                }

                *item = Value::from(new_addr.to_string());
            }
        }

        // Update boot nodes if provided
        if !self.boot_nodes.is_empty() {
            let list_array = doc["bootstrap"]["nodes"]
                .as_array_mut()
                .ok_or(eyre!("No swarm table in config.toml"))?;
            list_array.clear();
            for node in self.boot_nodes.iter() {
                list_array.push(node.to_string());
            }
        } else if let Some(network) = self.boot_network {
            let list_array = doc["bootstrap"]["nodes"]
                .as_array_mut()
                .ok_or(eyre!("No swarm table in config.toml"))?;
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
        if let Some(opts) = self.mdns {
            doc["discovery"]["mdns"] = toml_edit::value(opts.mdns);
        }

        // Save the updated TOML back to the file
        fs::write(&path, doc.to_string())?;

        info!("Node configuration has been updated");

        Ok(())
    }
}
