use std::fs;

use color_eyre::eyre::{self, Context};
use libp2p::{identity, Multiaddr};
use tracing::{info, warn};

use crate::config::{BootstrapConfig, Config, DiscoveryConfig, EndpointConfig, SwarmConfig};
use crate::config;

use crate::{BootstrapNodes, RootArgs, InitParams};

pub async fn run(args: RootArgs, init: InitParams) -> eyre::Result<()> {
    let mdns = init.mdns && !init.no_mdns;
    println!("{}",args.home);
    if !args.home.exists() {
        if args.home == config::default_chat_dir() {
            fs::create_dir_all(&args.home)
        } else {
            fs::create_dir(&args.home)
        }
        .wrap_err_with(|| format!("failed to create directory {:?}", args.home))?;
    }

    if Config::exists(&args.home) {
        if let Err(err) = Config::load(&args.home) {
            if init.force {
                warn!(
                    "Failed to load existing configuration, overwriting: {}",
                    err
                );
            } else {
                eyre::bail!("failed to load existing configuration: {}", err);
            }
        }
        if !init.force {
            eyre::bail!("chat node is already initialized in {:?}", args.home);
        }
    }

    let identity = identity::Keypair::generate_ed25519();
    info!("Generated identity: {:?}", identity.public().to_peer_id());

    let mut listen: Vec<Multiaddr> = vec![];

    for host in init.host {
        let host = format!(
            "/{}/{}",
            match host {
                std::net::IpAddr::V4(_) => "ip4",
                std::net::IpAddr::V6(_) => "ip6",
            },
            host,
        );
        listen.push(format!("{}/tcp/{}", host, init.port).parse()?);
        listen.push(format!("{}/udp/{}/quic-v1", host, init.port).parse()?);
    }

    let mut boot_nodes = init.boot_nodes;
    if let Some(BootstrapNodes::Ipfs) = init.boot_network {
        boot_nodes.extend(config::default_bootstrap());
    }

    let config = Config {
        identity,
        swarm: SwarmConfig { listen },
        bootstrap: BootstrapConfig { nodes: boot_nodes },
        discovery: DiscoveryConfig { mdns },
        endpoint: EndpointConfig {
            host: init.rpc_host,
            port: init.rpc_port,
        },
    };

    config.save(&args.home)?;

    info!("Initialized a chat node in {:?}", args.home);
    println!("Initialized a chat node in {:?}", args.home);
    Ok(())
}
