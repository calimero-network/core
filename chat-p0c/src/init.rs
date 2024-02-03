use std::fs;

use color_eyre::eyre::{self, Context};
use const_format::concatcp;
use libp2p::identity;
use tracing::info;

use crate::cli;
use crate::config::{BootstrapConfig, Config, SwarmConfig};

const DEFAULT_PORT: usize = 2428;

const DEFAULT_LISTEN: &[&str] = &[
    concatcp!("/ip4/0.0.0.0/tcp/", DEFAULT_PORT),
    concatcp!("/ip6/::/tcp/", DEFAULT_PORT),
    concatcp!("/ip4/0.0.0.0/udp/", DEFAULT_PORT, "/quic-v1"),
    concatcp!("/ip6/::/udp/", DEFAULT_PORT, "/quic-v1"),
];

pub async fn run(args: cli::RootArgs, init: cli::InitCommand) -> eyre::Result<()> {
    if !args.home.exists() {
        if args.home == cli::default_chat_dir() {
            fs::create_dir_all(&args.home)
        } else {
            fs::create_dir(&args.home)
        }
        .wrap_err_with(|| format!("failed to create directory {:?}", args.home))?;
    }

    if Config::exists(&args.home) {
        Config::load(&args.home)?;
        if !init.force {
            eyre::bail!("chat-p0c is already initialized in {:?}", args.home);
        }
    }

    let identity = identity::Keypair::generate_ed25519();
    info!("Generated identity: {:?}", identity.public().to_peer_id());

    let config = Config {
        identity,
        swarm: SwarmConfig {
            listen: DEFAULT_LISTEN
                .iter()
                .map(|addr| addr.parse().expect("invalid default listen address"))
                .collect(),
        },
        bootstrap: BootstrapConfig {
            nodes: init.boot_nodes,
        },
    };

    config.save(&args.home)?;

    Ok(())
}
