use std::collections::hash_map::{self, HashMap};
use std::collections::HashSet;

use color_eyre::eyre;
use color_eyre::owo_colors::OwoColorize;
use libp2p::futures::prelude::*;
use libp2p::multiaddr::{self, Multiaddr};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm, SwarmEvent};
use libp2p::{identify, kad, mdns, ping, relay, PeerId};
use tokio::sync::{mpsc, oneshot};
use tokio::time;
use tracing::{debug, info, trace, warn};

use crate::cli;
use crate::config::Config;

#[path = "events/mod.rs"]
mod events;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    relay: relay::Behaviour,
    ping: ping::Behaviour,
}

pub async fn run(args: cli::RootArgs) -> eyre::Result<()> {
    if !Config::exists(&args.home) {
        eyre::bail!("chat node is not initialized in {:?}", args.home);
    }

    let config = Config::load(&args.home)?;

    println!("{:?}", config);

    let peer_id = config.identity.public().to_peer_id();

    info!("Peer ID: {}", peer_id);

    let (mut client, event_loop) = init(peer_id, &config).await?;

    tokio::spawn(event_loop.run());

    for addr in &config.swarm.listen {
        client.listen_on(addr.clone()).await?;
    }

    client.bootstrap(config.bootstrap.nodes.clone()).await?;

    loop {
        tokio::select! {
            event = client.next_event() => match event {
                Some(event) => println!("Received event: {:?}", event),
                None => break,
            },
        }
    }

    Ok(())
}

async fn init(peer_id: PeerId, config: &Config) -> eyre::Result<(Client, EventLoop)> {
    let swarm = libp2p::SwarmBuilder::with_existing_identity(config.identity.clone())
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
            mdns: config
                .discovery
                .mdns
                .then_some(())
                .and_then(|_| mdns::Behaviour::new(mdns::Config::default(), peer_id).ok())
                .into(),
            kad: kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id)),
            relay: relay::Behaviour::new(peer_id, relay::Config::default()),
            ping: ping::Behaviour::default(),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(time::Duration::from_secs(30)))
        .build();

    let (command_sender, command_receiver) = mpsc::channel(32);
    let (event_sender, event_receiver) = mpsc::channel(32);

    let client = Client {
        sender: command_sender,
        receiver: event_receiver,
    };

    let event_loop = EventLoop::new(swarm, command_receiver, event_sender);

    Ok((client, event_loop))
}

pub(crate) struct Client {
    sender: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Event>,
}

impl Client {
    pub(crate) async fn next_event(&mut self) -> Option<Event> {
        self.receiver.recv().await
    }

    pub(crate) async fn listen_on(&mut self, addr: Multiaddr) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::ListenOn { addr, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }

    pub(crate) async fn bootstrap(&mut self, peer_addrs: Vec<Multiaddr>) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::Bootstrap { peer_addrs, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")?;

        Ok(())
    }

    pub(crate) async fn dial(&mut self, peer_addr: Multiaddr) -> eyre::Result<Option<()>> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::Dial { peer_addr, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }

    pub(crate) async fn start_providing(&mut self, key: String) {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::StartProviding { key, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.");
    }

    pub(crate) async fn get_providers(&mut self, key: String) -> HashSet<PeerId> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Command::GetProviders { key, sender })
            .await
            .expect("Command receiver not to be dropped.");
        receiver.await.expect("Sender not to be dropped.")
    }
}

pub(crate) struct EventLoop {
    swarm: Swarm<Behaviour>,
    command_receiver: mpsc::Receiver<Command>,
    event_sender: mpsc::Sender<Event>,
    pending_dial: HashMap<PeerId, oneshot::Sender<eyre::Result<Option<()>>>>,
    pending_bootstrap: HashMap<kad::QueryId, oneshot::Sender<eyre::Result<Option<()>>>>,
    pending_start_providing: HashMap<kad::QueryId, oneshot::Sender<()>>,
    pending_get_providers: HashMap<kad::QueryId, oneshot::Sender<HashSet<PeerId>>>,
}

impl EventLoop {
    fn new(
        swarm: Swarm<Behaviour>,
        command_receiver: mpsc::Receiver<Command>,
        event_sender: mpsc::Sender<Event>,
    ) -> Self {
        Self {
            swarm,
            command_receiver,
            event_sender,
            pending_dial: Default::default(),
            pending_bootstrap: Default::default(),
            pending_start_providing: Default::default(),
            pending_get_providers: Default::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        let mut interval = time::interval(time::Duration::from_secs(2));
        loop {
            tokio::select! {
                event = self.swarm.next() => self.handle_swarm_event(event.expect("Swarm stream to be infinite.")).await,
                command = self.command_receiver.recv() => match command {
                    Some(c) => self.handle_command(c).await,
                    None => break,
                },
                _ = interval.tick() => {
                    info!("{} peers", self.swarm.connected_peers().count());
                    // info!("{} peers, {:#?} in DHT", self.swarm.connected_peers().count(), self.swarm.behaviour_mut().kad.kbuckets().map(|e| e.iter().map(|f| (f.node.key.clone(), f.node.value.clone())).collect::<HashMap<_, _>>()).collect::<Vec<_>>());
                }
            }
        }
    }

    async fn handle_behaviour_event(&mut self, event: BehaviourEvent) {
        match event {
            BehaviourEvent::Identify(event) => events::EventHandler::handle(self, event).await,
            BehaviourEvent::Kad(event) => events::EventHandler::handle(self, event).await,
            BehaviourEvent::Mdns(event) => events::EventHandler::handle(self, event).await,
            BehaviourEvent::Gossipsub(event) => events::EventHandler::handle(self, event).await,
            BehaviourEvent::Relay(event) => events::EventHandler::handle(self, event).await,
            BehaviourEvent::Ping(event) => events::EventHandler::handle(self, event).await,
        }
    }

    async fn handle_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::Behaviour(behaviour) => self.handle_behaviour_event(behaviour).await,
            SwarmEvent::NewListenAddr { address, .. } => {
                let local_peer_id = *self.swarm.local_peer_id();
                info!(
                    "Listening on {}",
                    address.with(multiaddr::Protocol::P2p(local_peer_id))
                );
            }
            SwarmEvent::IncomingConnection { .. } => {}
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                if endpoint.is_dialer() {
                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        let _ = sender.send(Ok(Some(())));
                    }
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                endpoint,
                num_established,
                cause,
            } => {
                debug!(
                    "Connection closed: {} {:?} {:?} {} {:?}",
                    peer_id, connection_id, endpoint, num_established, cause
                );
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(peer_id) = peer_id {
                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        let _ = sender.send(Err(eyre::eyre!(error)));
                    }
                }
            }
            SwarmEvent::IncomingConnectionError { .. } => {}
            SwarmEvent::Dialing {
                peer_id: Some(peer_id),
                ..
            } => debug!("Dialing peer: {}", peer_id),
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                trace!("Expired listen address: {}", address)
            }
            SwarmEvent::ListenerClosed {
                addresses, reason, ..
            } => trace!("Listener closed: {:?} {:?}", addresses, reason.err()),
            SwarmEvent::ListenerError { error, .. } => trace!("Listener error: {:?}", error),
            SwarmEvent::NewExternalAddrCandidate { address } => {
                trace!("New external address candidate: {}", address)
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                trace!("External address confirmed: {}", address)
            }
            SwarmEvent::ExternalAddrExpired { address } => {
                trace!("External address expired: {}", address)
            }
            unhandled => warn!("Unhandled event: {:?}", unhandled),
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
            Command::Bootstrap { peer_addrs, sender } => {
                if peer_addrs.is_empty() {
                    let _ = sender.send(Ok(None));
                    return;
                }

                for mut addr in peer_addrs {
                    let Some(multiaddr::Protocol::P2p(peer_id)) = addr.pop() else {
                        let _ = sender
                            .send(Err(eyre::eyre!(format!("No peer ID in address: {}", addr))));
                        return;
                    };

                    self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                }

                match self.swarm.behaviour_mut().kad.bootstrap() {
                    Ok(query_id) => {
                        self.pending_bootstrap.insert(query_id, sender);
                    }
                    Err(err) => {
                        sender
                            .send(Err(eyre::eyre!(err)))
                            .expect("Receiver not to be dropped.");
                        return;
                    }
                }
            }
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
        peer_addrs: Vec<Multiaddr>,
        sender: oneshot::Sender<eyre::Result<Option<()>>>,
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

#[derive(Debug)]
pub(crate) enum Event {}
