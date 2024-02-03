use std::time;

use color_eyre::eyre;
use libp2p::futures::prelude::*;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::{identify, mdns, ping, relay, swarm};
use tracing::info;

use crate::cli;
use crate::config::Config;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(swarm::NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    relay: relay::Behaviour,
    ping: ping::Behaviour,
}

pub async fn run(args: cli::RootArgs) -> eyre::Result<()> {
    if !Config::exists(&args.home) {
        eyre::bail!("chat node is not initialized in {:?}", args.home);
    }

    let config = Config::load(&args.home)?;

    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let mut swarm = libp2p::SwarmBuilder::with_existing_identity(config.identity)
        .with_tokio()
        .with_tcp(
            Default::default(),
            (libp2p::tls::Config::new, libp2p::noise::Config::new),
            libp2p::yamux::Config::default,
        )?
        .with_quic()
        .with_behaviour(|key| Behaviour {
            identify: identify::Behaviour::new(identify::Config::new(
                PROTOCOL_VERSION.to_owned(),
                key.public(),
            )),
            mdns: mdns::Behaviour::new(mdns::Config::default(), peer_id)
                .ok()
                .into(),
            relay: relay::Behaviour::new(peer_id, relay::Config::default()),
            ping: ping::Behaviour::default(),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(time::Duration::from_secs(30)))
        .build();

    for addr in &config.swarm.listen {
        swarm.listen_on(addr.clone())?;
    }

    loop {
        match swarm.select_next_some().await {
            swarm::SwarmEvent::NewListenAddr { address, .. } => {
                info!("Listening on {}", address)
            }
            event => println!("{:?}", event),
        }
    }
}
