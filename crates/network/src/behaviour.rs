use std::time::Duration;

use calimero_network_primitives::config::NetworkConfig;
use eyre::WrapErr;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm};
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, rendezvous, tcp, tls, yamux,
    StreamProtocol, SwarmBuilder,
};
use multiaddr::Protocol;
use tracing::warn;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub dcutr: dcutr::Behaviour,
    pub gossipsub: gossipsub::Behaviour,
    pub identify: identify::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub mdns: Toggle<mdns::tokio::Behaviour>,
    pub ping: ping::Behaviour,
    pub relay: relay::client::Behaviour,
    pub rendezvous: rendezvous::client::Behaviour,
    pub stream: libp2p_stream::Behaviour,
}

impl Behaviour {
    pub fn build_swarm(config: &NetworkConfig) -> eyre::Result<Swarm<Self>> {
        let peer_id = config.identity.public().to_peer_id();

        let bootstrap_peers = {
            let mut peers = vec![];

            for mut addr in config.bootstrap.nodes.list.iter().cloned() {
                let Some(Protocol::P2p(peer_id)) = addr.pop() else {
                    eyre::bail!("Failed to parse peer id from addr {:?}", addr);
                };

                peers.push((peer_id, addr));
            }

            peers
        };

        let mut swarm = SwarmBuilder::with_existing_identity(config.identity.clone())
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                (tls::Config::new, noise::Config::new),
                yamux::Config::default,
            )?
            .with_quic()
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(|key, relay_behaviour| {
                let behaviour = Behaviour {
                    dcutr: dcutr::Behaviour::new(peer_id),
                    identify: identify::Behaviour::new(
                        identify::Config::new(PROTOCOL_VERSION.to_owned(), key.public())
                            .with_push_listen_addr_updates(true),
                    ),
                    mdns: config
                        .discovery
                        .mdns
                        .then_some(())
                        .map(|()| mdns::Behaviour::new(mdns::Config::default(), peer_id))
                        .transpose()?
                        .into(),
                    kad: {
                        let mut kad_config = kad::Config::default();
                        let _ = kad_config.set_protocol_names(vec![CALIMERO_KAD_PROTO_NAME]);

                        let mut kad = kad::Behaviour::with_config(
                            peer_id,
                            kad::store::MemoryStore::new(peer_id),
                            kad_config,
                        );

                        kad.set_mode(Some(kad::Mode::Client));

                        for (peer_id, addr) in bootstrap_peers {
                            let _ = kad.add_address(&peer_id, addr);
                        }

                        if let Err(err) = kad.bootstrap() {
                            warn!(%err, "Failed to bootstrap Kademlia");
                        };

                        kad
                    },
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key.clone()),
                        gossipsub::Config::default(),
                    )?,
                    ping: ping::Behaviour::default(),
                    rendezvous: rendezvous::client::Behaviour::new(key.clone()),
                    relay: relay_behaviour,
                    stream: libp2p_stream::Behaviour::new(),
                };

                Ok(behaviour)
            })?
            .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30)))
            .build();

        for addr in &config.swarm.listen {
            let _ignored = swarm
                .listen_on(addr.clone())
                .wrap_err_with(|| format!("failed to listen on '{}'", addr))?;
        }

        Ok(swarm)
    }
}
