use std::collections::hash_map::{self, HashMap};
use std::collections::HashSet;

use libp2p::futures::prelude::*;
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm, SwarmEvent};
use libp2p::{
    dcutr, gossipsub, identify, kad, mdns, noise, ping, relay, rendezvous, yamux, PeerId,
};
use multiaddr::Multiaddr;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, trace, warn};

pub mod client;
pub mod config;
mod discovery;
mod events;
pub mod stream;
pub mod types;

use client::NetworkClient;
use config::NetworkConfig;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: libp2p::StreamProtocol =
    libp2p::StreamProtocol::new("/calimero/kad/1.0.0");

#[derive(NetworkBehaviour)]
struct Behaviour {
    dcutr: dcutr::Behaviour,
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    mdns: Toggle<mdns::tokio::Behaviour>,
    ping: ping::Behaviour,
    rendezvous: rendezvous::client::Behaviour,
    relay: relay::client::Behaviour,
    stream: libp2p_stream::Behaviour,
}

pub async fn run(
    config: &NetworkConfig,
) -> eyre::Result<(NetworkClient, mpsc::Receiver<types::NetworkEvent>)> {
    let peer_id = config.identity.public().to_peer_id();

    let (client, event_receiver, event_loop) = init(peer_id, config).await?;

    tokio::spawn(event_loop.run());

    for addr in &config.swarm.listen {
        client.listen_on(addr.clone()).await?;
    }

    let _ = client.bootstrap().await;

    Ok((client, event_receiver))
}

async fn init(
    peer_id: PeerId,
    config: &NetworkConfig,
) -> eyre::Result<(
    NetworkClient,
    mpsc::Receiver<types::NetworkEvent>,
    EventLoop,
)> {
    let bootstrap_peers = {
        let mut peers = vec![];

        for mut addr in config.bootstrap.nodes.list.iter().cloned() {
            let Some(multiaddr::Protocol::P2p(peer_id)) = addr.pop() else {
                eyre::bail!("Failed to parse peer id from addr {:?}", addr);
            };

            peers.push((peer_id, addr));
        }

        peers
    };

    let swarm = libp2p::SwarmBuilder::with_existing_identity(config.identity.clone())
        .with_tokio()
        .with_tcp(
            Default::default(),
            (libp2p::tls::Config::new, libp2p::noise::Config::new),
            libp2p::yamux::Config::default,
        )?
        .with_quic()
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|key, relay_behaviour| Behaviour {
            dcutr: dcutr::Behaviour::new(peer_id.clone()),
            identify: identify::Behaviour::new(
                identify::Config::new(PROTOCOL_VERSION.to_owned(), key.public())
                    .with_push_listen_addr_updates(true),
            ),
            mdns: config
                .discovery
                .mdns
                .then_some(())
                .and_then(|_| mdns::Behaviour::new(mdns::Config::default(), peer_id).ok())
                .into(),
            kad: {
                let mut kad_config = kad::Config::default();
                kad_config.set_protocol_names(vec![CALIMERO_KAD_PROTO_NAME]);

                let mut kad = kad::Behaviour::with_config(
                    peer_id,
                    kad::store::MemoryStore::new(peer_id),
                    kad_config,
                );

                kad.set_mode(Some(kad::Mode::Client));

                for (peer_id, addr) in bootstrap_peers {
                    kad.add_address(&peer_id, addr);
                }
                if let Err(err) = kad.bootstrap() {
                    warn!(%err, "Failed to bootstrap Kademlia");
                };

                kad
            },
            gossipsub: gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub::Config::default(),
            )
            .expect("Valid gossipsub config."),
            ping: ping::Behaviour::default(),
            relay: relay_behaviour,
            rendezvous: rendezvous::client::Behaviour::new(key.clone()),
            stream: libp2p_stream::Behaviour::new(),
        })?
        .with_swarm_config(|cfg| {
            cfg.with_idle_connection_timeout(tokio::time::Duration::from_secs(30))
        })
        .build();

    let incoming_streams = match swarm
        .behaviour()
        .stream
        .new_control()
        .accept(stream::CALIMERO_STREAM_PROTOCOL)
    {
        Ok(incoming_streams) => incoming_streams,
        Err(err) => {
            eyre::bail!("Failed to setup control for stream protocol: {:?}", err)
        }
    };

    let (command_sender, command_receiver) = mpsc::channel(32);
    let (event_sender, event_receiver) = mpsc::channel(32);

    let client = NetworkClient {
        catchup_config: config.catchup.clone(),
        sender: command_sender,
    };

    let discovery = discovery::Discovery::new(&config.discovery.rendezvous);

    let event_loop = EventLoop::new(
        swarm,
        incoming_streams,
        command_receiver,
        event_sender,
        discovery,
    );

    Ok((client, event_receiver, event_loop))
}

pub(crate) struct EventLoop {
    swarm: Swarm<Behaviour>,
    incoming_streams: libp2p_stream::IncomingStreams,
    command_receiver: mpsc::Receiver<Command>,
    event_sender: mpsc::Sender<types::NetworkEvent>,
    discovery: discovery::Discovery,
    pending_dial: HashMap<PeerId, oneshot::Sender<eyre::Result<Option<()>>>>,
    pending_bootstrap: HashMap<kad::QueryId, oneshot::Sender<eyre::Result<Option<()>>>>,
    pending_start_providing: HashMap<kad::QueryId, oneshot::Sender<()>>,
    pending_get_providers: HashMap<kad::QueryId, oneshot::Sender<HashSet<PeerId>>>,
}

impl EventLoop {
    fn new(
        swarm: Swarm<Behaviour>,
        incoming_streams: libp2p_stream::IncomingStreams,
        command_receiver: mpsc::Receiver<Command>,
        event_sender: mpsc::Sender<types::NetworkEvent>,
        discovery: discovery::Discovery,
    ) -> Self {
        Self {
            swarm,
            incoming_streams,
            command_receiver,
            event_sender,
            discovery,
            pending_dial: Default::default(),
            pending_bootstrap: Default::default(),
            pending_start_providing: Default::default(),
            pending_get_providers: Default::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        let mut rendezvous_discover_tick =
            tokio::time::interval(self.discovery.rendezvous_config.discovery_interval);

        loop {
            tokio::select! {
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
                _ = rendezvous_discover_tick.tick() => self.handle_rendezvous_discoveries().await,
            }
        }
    }

    async fn handle_command(&mut self, command: Command) {
        match command {
            Command::ListenOn { addr, sender } => {
                let _ = match self.swarm.listen_on(addr) {
                    Ok(_) => sender.send(Ok(())),
                    Err(e) => sender.send(Err(eyre::eyre!(e))),
                };
            }
            Command::Bootstrap { sender } => match self.swarm.behaviour_mut().kad.bootstrap() {
                Ok(query_id) => {
                    self.pending_bootstrap.insert(query_id, sender);
                }
                Err(err) => {
                    sender
                        .send(Err(eyre::eyre!(err)))
                        .expect("Receiver not to be dropped.");
                    return;
                }
            },
            Command::Dial {
                mut peer_addr,
                sender,
            } => {
                let Some(multiaddr::Protocol::P2p(peer_id)) = peer_addr.pop() else {
                    let _ = sender.send(Err(eyre::eyre!(format!(
                        "No peer ID in address: {}",
                        peer_addr
                    ))));
                    return;
                };

                match self.pending_dial.entry(peer_id) {
                    hash_map::Entry::Occupied(_) => {
                        let _ = sender.send(Ok(None));
                    }
                    hash_map::Entry::Vacant(entry) => {
                        self.swarm
                            .behaviour_mut()
                            .kad
                            .add_address(&peer_id, peer_addr.clone());

                        match self.swarm.dial(peer_addr) {
                            Ok(()) => {
                                entry.insert(sender);
                            }
                            Err(e) => {
                                let _ = sender.send(Err(eyre::eyre!(e)));
                            }
                        }
                    }
                }
            }
            Command::Subscribe { topic, sender } => {
                if let Err(err) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
                    let _ = sender.send(Err(eyre::eyre!(err)));
                    return;
                }

                let _ = sender.send(Ok(topic));
            }
            Command::Unsubscribe { topic, sender } => {
                if let Err(err) = self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic) {
                    let _ = sender.send(Err(eyre::eyre!(err)));
                    return;
                }

                let _ = sender.send(Ok(topic));
            }
            Command::OpenStream { peer_id, sender } => {
                let _ = sender.send(self.open_stream(peer_id).await.map_err(Into::into));
            }
            Command::PeerCount { sender } => {
                let _ = sender.send(self.swarm.connected_peers().count());
            }
            Command::MeshPeerCount { topic, sender } => {
                let _ = sender.send(
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
                        let _ = sender.send(Err(eyre::eyre!(err)));
                        return;
                    }
                };

                let _ = sender.send(Ok(id));
            }
            Command::StartProviding { key, sender } => {
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .start_providing(key.into_bytes().into())
                    .expect("No store error.");
                self.pending_start_providing.insert(query_id, sender);
            }
            Command::GetProviders { key, sender } => {
                let query_id = self
                    .swarm
                    .behaviour_mut()
                    .kad
                    .get_providers(key.into_bytes().into());
                self.pending_get_providers.insert(query_id, sender);
            }
        }
    }
}

#[derive(Debug)]
enum Command {
    ListenOn {
        addr: Multiaddr,
        sender: oneshot::Sender<eyre::Result<()>>,
    },
    Dial {
        peer_addr: Multiaddr,
        sender: oneshot::Sender<eyre::Result<Option<()>>>,
    },
    Bootstrap {
        sender: oneshot::Sender<eyre::Result<Option<()>>>,
    },
    Subscribe {
        topic: gossipsub::IdentTopic,
        sender: oneshot::Sender<eyre::Result<gossipsub::IdentTopic>>,
    },
    Unsubscribe {
        topic: gossipsub::IdentTopic,
        sender: oneshot::Sender<eyre::Result<gossipsub::IdentTopic>>,
    },
    OpenStream {
        peer_id: PeerId,
        sender: oneshot::Sender<eyre::Result<stream::Stream>>,
    },
    PeerCount {
        sender: oneshot::Sender<usize>,
    },
    MeshPeerCount {
        topic: gossipsub::TopicHash,
        sender: oneshot::Sender<usize>,
    },
    Publish {
        topic: gossipsub::TopicHash,
        data: Vec<u8>,
        sender: oneshot::Sender<eyre::Result<gossipsub::MessageId>>,
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
