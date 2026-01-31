use core::time::Duration;

use calimero_network_primitives::config::NetworkConfig;
use calimero_network_primitives::specialized_node_invite::{
    SpecializedNodeInviteCodec, CALIMERO_SPECIALIZED_NODE_INVITE_PROTOCOL,
};
use eyre::WrapErr;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm};
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, rendezvous, tcp, tls, yamux,
    StreamProtocol, SwarmBuilder,
};
use multiaddr::Protocol;
use tracing::warn;

use crate::autonat;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

#[expect(
    missing_debug_implementations,
    reason = "Swarm behaviours don't implement Debug"
)]
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
    pub specialized_node_invite: request_response::Behaviour<SpecializedNodeInviteCodec>,
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
                            autonat::Config::default()
                                .with_max_candidates(config.discovery.autonat.max_candidates)
                                .with_probe_interval(config.discovery.autonat.probe_interval),
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

                        for (peer_id, addr) in bootstrap_peers {
                            let _ = kad.add_address(&peer_id, addr);
                        }

                        if let Err(err) = kad.bootstrap() {
                            warn!(%err, "Failed to bootstrap Kademlia");
                        }

                        kad
                    },
                    gossipsub: {
                        // Configure gossipsub with shorter backoff for faster mesh recovery
                        // after node restarts. Default is 60 seconds which blocks reconnection.
                        let gossipsub_config = gossipsub::ConfigBuilder::default()
                            // Reduce prune backoff from 60s to 5s for faster restart recovery
                            .prune_backoff(Duration::from_secs(5))
                            // Reduce graft flood threshold for faster mesh formation
                            .graft_flood_threshold(Duration::from_secs(5))
                            // Standard heartbeat interval
                            .heartbeat_interval(Duration::from_secs(1))
                            .build()
                            .expect("valid gossipsub config");

                        gossipsub::Behaviour::new(
                            gossipsub::MessageAuthenticity::Signed(key.clone()),
                            gossipsub_config,
                        )?
                    },
                    ping: ping::Behaviour::default(),
                    rendezvous: rendezvous::client::Behaviour::new(key.clone()),
                    relay: relay_behaviour,
                    stream: libp2p_stream::Behaviour::new(),
                    specialized_node_invite: request_response::Behaviour::new(
                        [(
                            CALIMERO_SPECIALIZED_NODE_INVITE_PROTOCOL,
                            ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
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
