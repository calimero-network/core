use std::fs;
use std::net::IpAddr;

use calimero_network::config::{BootstrapConfig, BootstrapNodes, DiscoveryConfig, SwarmConfig};
use calimero_node::config::{
    self, ApplicationConfig, ConfigFile, ConfigImpl, InitFile, NetworkConfig, StoreConfig,
};
use calimero_runtime::Constraint;
use camino::Utf8PathBuf;
use clap::parser::ValueSource;
use clap::{ArgMatches, Command, CommandFactory, Parser, ValueEnum};
use eyre::eyre;
use libp2p::identity;
use multiaddr::Multiaddr;
use tracing::{info, warn};

use crate::cli;

/// Initialize node configuration
#[derive(Debug, Parser)]
pub struct SetupCommand {
    /// Name of node
    #[arg(short, long, value_name = "NAME")]
    pub node_name: Utf8PathBuf,

    /// List of bootstrap nodes
    #[arg(long, value_name = "ADDR")]
    pub boot_nodes: Vec<Multiaddr>,

    /// Use nodes from a known network
    #[arg(long, value_name = "NETWORK", default_value = "calimero-dev")]
    pub boot_network: Option<BootstrapNetwork>,

    /// Host to listen on
    #[arg(
        long,
        value_name = "HOST",
        default_value = "0.0.0.0,::",
        use_value_delimiter = true
    )]
    pub swarm_host: Vec<IpAddr>,

    /// Port to listen on
    #[arg(long, value_name = "PORT", default_value_t = calimero_network::config::DEFAULT_PORT)]
    pub swarm_port: u16,

    /// Host to listen on for RPC
    #[arg(
        long,
        value_name = "HOST",
        default_value = "127.0.0.1,::1",
        use_value_delimiter = true
    )]
    pub server_host: Vec<IpAddr>,

    /// Port to listen on for RPC
    #[arg(long, value_name = "PORT", default_value_t = calimero_server::config::DEFAULT_PORT)]
    pub server_port: u16,

    /// Enable mDNS discovery
    #[arg(long, default_value_t = true, overrides_with("no_mdns"))]
    pub mdns: bool,

    #[arg(long, hide = true, overrides_with("mdns"))]
    pub no_mdns: bool,

    /// Force edit even if the argument already exists
    #[arg(long, short)]
    pub force: bool,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum BootstrapNetwork {
    CalimeroDev,
    Ipfs,
}

impl SetupCommand {
    pub fn run(mut self, root_args: cli::RootArgs) -> eyre::Result<()> {
        let path = root_args.home.join(&self.node_name);

        let matches = cli::RootCommand::command().get_matches(); // get matches for value_source

        // Extract subcommand matches for SetupCommand
        let setup_matches = matches.subcommand_matches("setup").unwrap();

        /// Znaci
        /// Ako se ucitao config, onda mijenjamo
        /// Ako se nije ucitao config, onda samo guras ovo sta je dano i ostalo default
        ///
        /// Ako se ucitao:
        ///     ako nema -f force, otpili ga
        ///     ako ima, provjeri samo za svaki dal je zastavica tu:
        ///         ako je, daj novi
        ///         ako nije, daj mu stari
        ///
        /// SPREMI
        let boot_nodes_provided = check_if_provided(&setup_matches, "boot_nodes");
        let boot_network_provided = check_if_provided(&setup_matches, "boot_network");
        let swarm_host_provided = check_if_provided(&setup_matches, "swarm_host");
        let swarm_port_provided = check_if_provided(&setup_matches, "swarm_port");
        let server_host_provided = check_if_provided(&setup_matches, "server_host");
        let server_port_provided = check_if_provided(&setup_matches, "server_port");
        let mdns_provided = check_if_provided(&setup_matches, "mdns");
        let no_mdns_provided = check_if_provided(&setup_matches, "no_mdns");

        // You can now use these boolean variables to check if the fields were provided
        println!("boot_nodes provided: {}", boot_nodes_provided);
        println!("swarm_host provided: {}", swarm_host_provided);
        println!("swarm_port provided: {}", swarm_port_provided);
        println!("server_host provided: {}", server_host_provided);
        println!("server_port provided: {}", server_port_provided);
        println!("mdns provided: {}", mdns_provided);
        println!("no_mdns provided: {}", no_mdns_provided);

        let identity;
        let mut swarm_listen = None;
        let mut server_listen = None;

        if InitFile::exists(&path) {
            if let Ok(config_identity) = InitFile::load(&path) {
                identity = config_identity.identity;
            } else {
                eyre::bail!("Failed to open identity for node \nRun command node init -n <NAME>");
            }
            if let Ok(config) = ConfigFile::load(&path) {
                if !self.force {
                    eyre::bail!(
                        "The config for the node is already initialized \nYou can override a setting by adding the -f flag"
                    );
                } else {
                    if !boot_network_provided {
                        self.boot_nodes = config.network.bootstrap.nodes.list;
                    }
                    if !swarm_host_provided && !swarm_port_provided {
                        swarm_listen = Some(config.network.swarm.listen);
                    }
                    if !server_host_provided && !server_port_provided {
                        server_listen = Some(config.network.server.listen);
                    }
                    if !mdns_provided {
                        self.mdns = config.network.discovery.mdns;
                    }
                }
            }
        } else {
            eyre::bail!("You have to initialize the node first \nRun command node init -n <NAME>");
        }

        //sada ako je postavljeno, i ides promijenit port
        //promijenit ce se port ali sa default hostom
        //al to je ok...?
        //jedino ako ne das ni jedno ni drugo, onda ostaje isto

        let mdns = self.mdns && !self.no_mdns;

        let mut listen: Vec<Multiaddr> = vec![];

        match swarm_listen {
            Some(data) => listen.extend(data),
            None => {
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
            }
        }

        let mut boot_nodes = vec![];
        if boot_network_provided {
            if let Some(network) = self.boot_network {
                match network {
                    BootstrapNetwork::CalimeroDev => {
                        boot_nodes.extend(BootstrapNodes::calimero_dev().list)
                    }
                    BootstrapNetwork::Ipfs => boot_nodes.extend(BootstrapNodes::ipfs().list),
                }
            }
        } else {
            boot_nodes = self.boot_nodes;
        }

        let config_new = ConfigFile {
            identity,
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
                server: calimero_node::config::ServerConfig {
                    listen: match server_listen {
                        Some(data) => data,
                        None => self
                            .server_host
                            .into_iter()
                            .map(|host| {
                                Multiaddr::from(host)
                                    .with(multiaddr::Protocol::Tcp(self.server_port))
                            })
                            .collect(),
                    },
                    admin: Some(calimero_server::admin::service::AdminConfig { enabled: true }),
                    jsonrpc: Some(calimero_server::jsonrpc::JsonRpcConfig { enabled: true }),
                    websocket: Some(calimero_server::ws::WsConfig { enabled: true }),
                },
            },
        };

        config_new.save(&path)?;

        calimero_store::Store::open(&calimero_store::config::StoreConfig {
            path: path.join(config_new.store.path),
        })?;

        info!("Initialized confing for a node in {:?}", root_args.home);

        Ok(())
    }
}

fn check_if_provided(matches: &ArgMatches, arg_name: &str) -> bool {
    if let Some(ValueSource::CommandLine) = matches.value_source(arg_name) {
        return true;
    }
    false
}
