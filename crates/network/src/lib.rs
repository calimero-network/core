#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]

use std::collections::hash_map::{Entry, HashMap};
use std::collections::HashSet;
use std::net::Ipv4Addr;

use client::NetworkClient;
use config::NetworkConfig;
use eyre::{bail, eyre, Result as EyreResult};
use libp2p::autonat::{Behaviour as AutonatBehaviour, Config as AutonatConfig};
use libp2p::dcutr::Behaviour as DcutrBehaviour;
use libp2p::futures::prelude::*;
use libp2p::gossipsub::{
    Behaviour as GossipsubBehaviour, Config as GossipsubConfig, IdentTopic, MessageAuthenticity,
    MessageId, TopicHash,
};
use libp2p::identify::{Behaviour as IdentifyBehaviour, Config as IdentifyConfig};
use libp2p::kad::store::MemoryStore;
use libp2p::kad::{Behaviour as KadBehaviour, Config as KadConfig, Mode, QueryId};
use libp2p::mdns::tokio::Behaviour as MdnsTokioBehaviour;
use libp2p::mdns::{Behaviour as MdnsBehaviour, Config as MdnsConfig};
use libp2p::noise::Config as NoiseConfig;
use libp2p::ping::Behaviour as PingBehaviour;
use libp2p::relay::client::Behaviour as RelayBehaviour;
use libp2p::rendezvous::client::Behaviour as RendezvousBehaviour;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm, SwarmEvent};
use libp2p::tcp::Config as TcpConfig;
use libp2p::tls::Config as TlsConfig;
use libp2p::yamux::Config as YamuxConfig;
use libp2p::{PeerId, StreamProtocol, SwarmBuilder};
use libp2p_stream::{Behaviour as StreamBehaviour, IncomingStreams};
use multiaddr::{Multiaddr, Protocol};
use reqwest::Client;
use stream::Stream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, Duration};
use tokio::{select, spawn};
use tracing::{debug, info, trace, warn};

use crate::discovery::Discovery;
use crate::types::NetworkEvent;

pub mod client;
pub mod config;
mod discovery;
mod events;
pub mod stream;
pub mod types;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

#[derive(NetworkBehaviour)]
struct Behaviour {
    autonat: AutonatBehaviour,
    dcutr: DcutrBehaviour,
    gossipsub: GossipsubBehaviour,
    identify: IdentifyBehaviour,
    kad: KadBehaviour<MemoryStore>,
    mdns: Toggle<MdnsTokioBehaviour>,
    ping: PingBehaviour,
    rendezvous: RendezvousBehaviour,
    relay: RelayBehaviour,
    stream: StreamBehaviour,
}

pub async fn run(
    config: &NetworkConfig,
) -> EyreResult<(NetworkClient, mpsc::Receiver<NetworkEvent>)> {
    let peer_id = config.identity.public().to_peer_id();

    let (client, event_receiver, event_loop) = init(peer_id, config).await?;

    drop(spawn(event_loop.run()));

    let mut ports = HashSet::new();
    for addr in &config.swarm.listen {
        info!("Listen addr: {:?}", addr);
        addr.iter().for_each(|p| {
            let port = match p {
                Protocol::Tcp(port) | Protocol::Udp(port) => port,
                _ => {
                    return;
                }
            };
            _ = ports.insert(port);
        });
        client.listen_on(addr.clone()).await?;
    }

    drop(client.bootstrap().await);

    Ok((client, event_receiver))
}

async fn init(
    peer_id: PeerId,
    config: &NetworkConfig,
) -> EyreResult<(NetworkClient, mpsc::Receiver<NetworkEvent>, EventLoop)> {
    let bootstrap_peers = {
        let mut peers = vec![];

        for mut addr in config.bootstrap.nodes.list.iter().cloned() {
            let Some(Protocol::P2p(peer_id)) = addr.pop() else {
                bail!("Failed to parse peer id from addr {:?}", addr);
            };

            peers.push((peer_id, addr));
        }

        peers
    };

    let swarm = SwarmBuilder::with_existing_identity(config.identity.clone())
        .with_tokio()
        .with_tcp(
            TcpConfig::default(),
            (TlsConfig::new, NoiseConfig::new),
            YamuxConfig::default,
        )?
        .with_quic()
        .with_relay_client(NoiseConfig::new, YamuxConfig::default)?
        .with_behaviour(|key, relay_behaviour| Behaviour {
            autonat: {
                AutonatBehaviour::new(
                    peer_id,
                    AutonatConfig {
                        boot_delay: Duration::from_secs(5),
                        ..Default::default()
                    },
                )
            },
            dcutr: DcutrBehaviour::new(peer_id),
            identify: IdentifyBehaviour::new(
                IdentifyConfig::new(PROTOCOL_VERSION.to_owned(), key.public())
                    .with_push_listen_addr_updates(true),
            ),
            mdns: config
                .discovery
                .mdns
                .then_some(())
                .and_then(|()| MdnsBehaviour::new(MdnsConfig::default(), peer_id).ok())
                .into(),
            kad: {
                let mut kad_config = KadConfig::default();
                let _ = kad_config.set_protocol_names(vec![CALIMERO_KAD_PROTO_NAME]);

                let mut kad =
                    KadBehaviour::with_config(peer_id, MemoryStore::new(peer_id), kad_config);

                kad.set_mode(Some(Mode::Client));

                for (peer_id, addr) in bootstrap_peers {
                    let _ = kad.add_address(&peer_id, addr);
                }
                if let Err(err) = kad.bootstrap() {
                    warn!(%err, "Failed to bootstrap Kademlia");
                };

                kad
            },
            gossipsub: GossipsubBehaviour::new(
                MessageAuthenticity::Signed(key.clone()),
                GossipsubConfig::default(),
            )
            .expect("Valid gossipsub config."),
            ping: PingBehaviour::default(),
            relay: relay_behaviour,
            rendezvous: RendezvousBehaviour::new(key.clone()),
            stream: StreamBehaviour::new(),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(Duration::from_secs(30)))
        .build();

    let incoming_streams = match swarm
        .behaviour()
        .stream
        .new_control()
        .accept(stream::CALIMERO_STREAM_PROTOCOL)
    {
        Ok(incoming_streams) => incoming_streams,
        Err(err) => {
            bail!("Failed to setup control for stream protocol: {:?}", err)
        }
    };

    let (command_sender, command_receiver) = mpsc::channel(32);
    let (event_sender, event_receiver) = mpsc::channel(32);

    let client = NetworkClient {
        sender: command_sender,
    };

    let discovery = Discovery::new(
        &config.discovery.rendezvous,
        &config.discovery.relay,
        &config.discovery.autonat,
    );

    let mut ports = HashSet::new();
    for addr in &config.swarm.listen {
        addr.iter().for_each(|p| {
            let port = match p {
                Protocol::Tcp(port) => port,
                Protocol::Udp(port) => port,
                _ => {
                    return;
                }
            };
            _ = ports.insert(port);
        });
    }
    let advertise_address = if config.discovery.advertise_address {
        Some(AdvertiseAddress {
            ip: get_public_ip().await?,
            ports,
        })
    } else {
        None
    };

    let event_loop = EventLoop::new(
        swarm,
        advertise_address,
        incoming_streams,
        command_receiver,
        event_sender,
        discovery,
    );

    Ok((client, event_receiver, event_loop))
}

async fn get_public_ip() -> EyreResult<Ipv4Addr> {
    let client = Client::builder().timeout(Duration::from_secs(3)).build()?;
    let ip_addr = client
        .get("https://api.ipify.org")
        .send()
        .await?
        .text()
        .await?
        .parse()?;

    return Ok(ip_addr);
}

pub(crate) struct EventLoop {
    swarm: Swarm<Behaviour>,
    advertise_address: Option<AdvertiseAddress>,
    incoming_streams: IncomingStreams,
    command_receiver: mpsc::Receiver<Command>,
    event_sender: mpsc::Sender<NetworkEvent>,
    discovery: Discovery,
    pending_dial: HashMap<PeerId, oneshot::Sender<EyreResult<Option<()>>>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<EyreResult<Option<()>>>>,
    pending_start_providing: HashMap<QueryId, oneshot::Sender<()>>,
    pending_get_providers: HashMap<QueryId, oneshot::Sender<HashSet<PeerId>>>,
}

pub(crate) struct AdvertiseAddress {
    ip: Ipv4Addr,
    ports: HashSet<u16>,
}

#[allow(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
impl EventLoop {
    fn new(
        swarm: Swarm<Behaviour>,
        advertise_public_ip: Option<AdvertiseAddress>,
        incoming_streams: IncomingStreams,
        command_receiver: mpsc::Receiver<Command>,
        event_sender: mpsc::Sender<NetworkEvent>,
        discovery: Discovery,
    ) -> Self {
        Self {
            swarm,
            advertise_address: advertise_public_ip,
            incoming_streams,
            command_receiver,
            event_sender,
            discovery,
            pending_dial: HashMap::default(),
            pending_bootstrap: HashMap::default(),
            pending_start_providing: HashMap::default(),
            pending_get_providers: HashMap::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        let mut rendezvous_discover_tick =
            interval(self.discovery.rendezvous_config.discovery_interval);

        #[expect(clippy::redundant_pub_crate, reason = "Needed for Tokio code")]
        loop {
            select! {
                event = self.swarm.next() => {
                    self.handle_swarm_event(event.expect("Swarm stream to be infinite.")).await;
                },
                incoming_stream = self.incoming_streams.next() => {
                    self.handle_incoming_stream(incoming_stream.expect("Incoming streams to be infinite.")).await;
                },
                command = self.command_receiver.recv() => {
                    let Some(c) = command else { break };
                    self.handle_command(c).await;
                }
                _ = rendezvous_discover_tick.tick() => self.broadcast_rendezvous_discoveries(),
            }
        }
    }

    // TODO: Consider splitting this long function into multiple parts.
    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::ListenOn { addr, sender } => {
                drop(match self.swarm.listen_on(addr) {
                    Ok(_) => sender.send(Ok(())),
                    Err(e) => sender.send(Err(eyre!(e))),
                });
            }
            Command::Bootstrap { sender } => match self.swarm.behaviour_mut().kad.bootstrap() {
                Ok(query_id) => {
                    drop(self.pending_bootstrap.insert(query_id, sender));
                }
                Err(err) => sender
                    .send(Err(eyre!(err)))
                    .expect("Receiver not to be dropped."),
            },
            Command::Dial {
                mut peer_addr,
                sender,
            } => {
                let Some(Protocol::P2p(peer_id)) = peer_addr.pop() else {
                    drop(sender.send(Err(eyre!(format!("No peer ID in address: {}", peer_addr)))));
                    return;
                };

                match self.pending_dial.entry(peer_id) {
                    Entry::Occupied(_) => {
                        drop(sender.send(Ok(None)));
                    }
                    Entry::Vacant(entry) => {
                        let _ = self
                            .swarm
                            .behaviour_mut()
                            .kad
                            .add_address(&peer_id, peer_addr.clone());

                        match self.swarm.dial(peer_addr) {
                            Ok(()) => {
                                let _ = entry.insert(sender);
                            }
                            Err(e) => {
                                drop(sender.send(Err(eyre!(e))));
                            }
                        }
                    }
                }
            }
            Command::Subscribe { topic, sender } => {
                if let Err(err) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
                    drop(sender.send(Err(eyre!(err))));
                    return;
                }

                drop(sender.send(Ok(topic)));
            }
            Command::Unsubscribe { topic, sender } => {
                if let Err(err) = self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic) {
                    drop(sender.send(Err(eyre!(err))));
                    return;
                }

                drop(sender.send(Ok(topic)));
            }
            Command::OpenStream { peer_id, sender } => {
                drop(sender.send(self.open_stream(peer_id).await.map_err(Into::into)));
            }
            Command::PeerCount { sender } => {
                let _ignore = sender.send(self.swarm.connected_peers().count());
            }
            Command::MeshPeers { topic, sender } => {
                drop(
                    sender.send(
                        self.swarm
                            .behaviour_mut()
                            .gossipsub
                            .mesh_peers(&topic)
                            .copied()
                            .collect(),
                    ),
                );
            }
            Command::MeshPeerCount { topic, sender } => {
                let _ignore = sender.send(
                    self.swarm
                        .behaviour_mut()
                        .gossipsub
                        .mesh_peers(&topic)
                        .count(),
                );
            }
            Command::Publish {
                topic,
                data,
                sender,
            } => {
                let id = match self.swarm.behaviour_mut().gossipsub.publish(topic, data) {
                    Ok(id) => id,
                    Err(err) => {
                        drop(sender.send(Err(eyre!(err))));
                        return;
                    }
                };

                drop(sender.send(Ok(id)));
            }
            Command::StartProviding { key, sender } => {
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .start_providing(key.into_bytes().into())
                    .expect("No store error.");
                drop(self.pending_start_providing.insert(query_id, sender));
            }
            Command::GetProviders { key, sender } => {
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .get_providers(key.into_bytes().into());
                drop(self.pending_get_providers.insert(query_id, sender));
            }
        }
    }
}

#[derive(Debug)]
enum Command {
    ListenOn {
        addr: Multiaddr,
        sender: oneshot::Sender<EyreResult<()>>,
    },
    Dial {
        peer_addr: Multiaddr,
        sender: oneshot::Sender<EyreResult<Option<()>>>,
    },
    Bootstrap {
        sender: oneshot::Sender<EyreResult<Option<()>>>,
    },
    Subscribe {
        topic: IdentTopic,
        sender: oneshot::Sender<EyreResult<IdentTopic>>,
    },
    Unsubscribe {
        topic: IdentTopic,
        sender: oneshot::Sender<EyreResult<IdentTopic>>,
    },
    OpenStream {
        peer_id: PeerId,
        sender: oneshot::Sender<EyreResult<Stream>>,
    },
    PeerCount {
        sender: oneshot::Sender<usize>,
    },
    MeshPeerCount {
        topic: TopicHash,
        sender: oneshot::Sender<usize>,
    },
    MeshPeers {
        topic: TopicHash,
        sender: oneshot::Sender<Vec<PeerId>>,
    },
    Publish {
        topic: TopicHash,
        data: Vec<u8>,
        sender: oneshot::Sender<EyreResult<MessageId>>,
    },
    StartProviding {
        key: String,
        sender: oneshot::Sender<()>,
    },
    GetProviders {
        key: String,
        sender: oneshot::Sender<HashSet<PeerId>>,
    },
}
