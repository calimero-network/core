use core::time::Duration;

use calimero_network_primitives::config::NetworkConfig;
use eyre::WrapErr;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm};
use libp2p::{
    autonat, dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, rendezvous, tcp, tls,
    yamux, StreamProtocol, SwarmBuilder,
};
use multiaddr::Protocol;
use tracing::warn;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    pub autonat: autonat::Behaviour,
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
                let behaviour = Self {
                    autonat: {
                        autonat::Behaviour::new(
                            peer_id,
                            autonat::Config {
                                boot_delay: Duration::from_secs(5),
                                ..Default::default()
                            },
                        )
                    },
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
                        let kad_config = kad::Config::new(CALIMERO_KAD_PROTO_NAME);

                        let mut kad = kad::Behaviour::with_config(
                            peer_id,
                            kad::store::MemoryStore::new(peer_id),
                            kad_config,
                        );

                        kad.set_mode(Some(kad::Mode::Server));

                        for (peer_id, addr) in &bootstrap_peers {
                            let _ = kad.add_address(&peer_id, addr.clone());
                        }

                        if let Err(err) = kad.bootstrap() {
                            warn!(%err, "Failed to bootstrap Kademlia");
                        }

                        kad
                    },
                    gossipsub: {
                        // Create adaptive gossipsub configuration
                        let gossipsub_config = if config.gossipsub.adaptive_mesh {
                            // Adaptive mesh configuration based on expected network size
                            let expected_peers = bootstrap_peers.len().max(2); // At least 2 peers expected
                            
                            let (mesh_n_low, mesh_n, mesh_n_high, heartbeat_interval) = match expected_peers {
                                0..=2 => (1, 2, 3, Duration::from_millis(500)),    // Tiny networks: 1-2 mesh peers, fast heartbeat
                                3..=5 => (2, 3, 4, Duration::from_millis(700)),    // Small networks: 2-3 mesh peers
                                6..=10 => (3, 4, 6, Duration::from_secs(1)),       // Medium networks: 3-4 mesh peers
                                11..=20 => (4, 6, 8, Duration::from_secs(1)),      // Large networks: 4-6 mesh peers
                                _ => (6, 8, 12, Duration::from_secs(1)),           // Very large networks: 6-8 mesh peers
                            };
                            
                            gossipsub::ConfigBuilder::default()
                                .mesh_n_low(mesh_n_low)
                                .mesh_n(mesh_n)
                                .mesh_n_high(mesh_n_high)
                                .heartbeat_interval(heartbeat_interval)
                                .build()
                                .expect("Valid gossipsub config")
                        } else {
                            // Use manual configuration or defaults
                            let mut builder = gossipsub::ConfigBuilder::default();
                            
                            if let Some(mesh_n_low) = config.gossipsub.mesh_n_low {
                                builder.mesh_n_low(mesh_n_low);
                            }
                            if let Some(mesh_n) = config.gossipsub.mesh_n {
                                builder.mesh_n(mesh_n);
                            }
                            if let Some(mesh_n_high) = config.gossipsub.mesh_n_high {
                                builder.mesh_n_high(mesh_n_high);
                            }
                            if let Some(heartbeat_ms) = config.gossipsub.heartbeat_interval_ms {
                                builder.heartbeat_interval(Duration::from_millis(heartbeat_ms));
                            }
                            
                            builder.build().expect("Valid gossipsub config")
                        };
                        
                        gossipsub::Behaviour::new(
                            gossipsub::MessageAuthenticity::Signed(key.clone()),
                            gossipsub_config,
                        )?
                    },
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
                .wrap_err_with(|| format!("failed to listen on '{addr}'"))?;
        }

        Ok(swarm)
    }
}
