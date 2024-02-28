use std::collections::hash_map::{self, HashMap};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use axum::response::IntoResponse;
use axum::routing::{get_service, Router};
use color_eyre::eyre;
use color_eyre::owo_colors::OwoColorize;
use jsonrpsee::server::stop_channel;
use libp2p::futures::prelude::*;
use libp2p::multiaddr::{self, Multiaddr};
use libp2p::swarm::behaviour::toggle::Toggle;
use libp2p::swarm::{NetworkBehaviour, Swarm, SwarmEvent};
use libp2p::{gossipsub, identify, kad, mdns, ping, relay, PeerId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncBufReadExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time;
use tracing::{debug, error, info, trace, warn};

use crate::cli::{self, NodeType};
use crate::config::Config;
use crate::endpoint::{self, CalimeroRPCServer};

mod events;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(NetworkBehaviour)]
struct Behaviour {
    identify: identify::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    relay: relay::Behaviour,
    ping: ping::Behaviour,
}

struct Storage {
    transactions: Arc<Mutex<HashMap<Hash, Transaction>>>,
    confirmations: Arc<Mutex<HashMap<Hash, Transaction>>>,
    senders: Arc<Mutex<HashMap<Hash, PeerId>>>,
    last_known_transaction_hash: Arc<Mutex<Hash>>,
    nonce: u64,
    node_type: NodeType,

    peer_id: PeerId,
    chosen_coordinator: Arc<Mutex<CoordinatorState>>,
    last_network_event_timestamp: SystemTime,
}

pub async fn run(args: cli::RootArgs) -> eyre::Result<()> {
    if !Config::exists(&args.home) {
        eyre::bail!("chat node is not initialized in {:?}", args.home);
    }

    let config: Config = Config::load(&args.home)?;

    let addr: std::net::SocketAddr =
        format!("{}:{}", config.endpoint.host, config.endpoint.port).parse()?;

    tokio::spawn(async move {
        let (stop_handle, _server_handle) = stop_channel();
        let service_builder = jsonrpsee::server::ServerBuilder::new().to_service_builder();

        let server =
            service_builder.build(endpoint::CalimeroRPCImpl::new().into_rpc(), stop_handle);

        let app = Router::new().route(
            "/",
            get_service(server).handle_error(
                |err: Box<dyn std::error::Error + Send + Sync>| async move {
                    err.to_string().into_response()
                },
            ),
        );

        axum::Server::bind(&addr)
            .serve(app.into_make_service())
            .await
            .unwrap();
    });

    // Setup the P2P network
    let peer_id = config.identity.public().to_peer_id();
    info!("Peer ID: {}", peer_id);

    let mut storage = Storage {
        transactions: Arc::new(Mutex::new(HashMap::new())),
        confirmations: Arc::new(Mutex::new(HashMap::new())),
        senders: Arc::new(Mutex::new(HashMap::new())),
        last_known_transaction_hash: Default::default(),
        nonce: 0,
        node_type: args.node_type,
        peer_id,

        chosen_coordinator: Arc::new(Mutex::new(CoordinatorState::None)),
        last_network_event_timestamp: SystemTime::now(),
    };

    let (mut client, mut event_receiver, event_loop) = init(peer_id, &config).await?;
    tokio::spawn(event_loop.run());

    for addr in &config.swarm.listen {
        client.listen_on(addr.clone()).await?;
    }

    let _ = client.bootstrap().await;

    let application_topic =
        gossipsub::IdentTopic::new("/calimero/experimental/chat-p0c".to_owned());

    let coordinators_topic =
        gossipsub::IdentTopic::new("/calimero/experimental/coordinators".to_owned());

    // coordinator should join only on request
    if !storage.node_type.is_coordinator() {
        join_topic(&mut client, &application_topic).await?;
    }

    // join leader and coordinators to topic
    if !storage.node_type.is_peer() {
        join_topic(&mut client, &coordinators_topic).await?;
    }

    if !storage.node_type.is_coordinator()
        && client.mesh_peer_count(application_topic.hash()).await != 0
    {
        client
            .publish(
                application_topic.hash(),
                serde_json::to_vec(&NetworkAction::WhoIsCoordinator)?,
            )
            .await?;
    }

    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    //let handler = handler_read.clone();

    loop {
        tokio::select! {
            _ = choose_coordinator(client.clone(), &mut storage, application_topic.hash(), coordinators_topic.clone()), if !storage.node_type.is_coordinator() => {

            }
            event = event_receiver.recv() => {
                match event {
                    Some(event) => {
                        if storage.node_type.is_coordinator() {
                            coordinator_event_recipient(client.clone(), application_topic.clone(), event, &mut storage, coordinators_topic.hash()).await?
                        } else {
                            peer_event_recipient(client.clone(), application_topic.hash(), event, &mut storage, coordinators_topic.clone()).await?
                        }
                    },
                    None => break,
                }
            }
            line = stdin.next_line() => {
                match line {
                    Ok(Some(line)) => {
                        if storage.node_type.is_coordinator() {
                            error!("Coordinator can not create transactions!");
                            continue;
                        }
                        if client.mesh_peer_count(application_topic.hash()).await == 0 {
                            info!("No connected peers to send message to.");
                            continue;
                        }
                        client
                            .publish(application_topic.hash(), serde_json::to_vec(&create_transaction(&line, &storage)?)?)
                            .await
                            .expect("Failed to publish message.");
                    }
                    Ok(None) => (),
                    Err(e) => eprintln!("Error popping from list: {:?}", e),
                }
            }
        }
    }

    Ok(())
}

async fn join_topic(client: &mut Client, topic: &gossipsub::IdentTopic) -> eyre::Result<()> {
    client.subscribe(topic.clone()).await?;

    Ok(())
}

async fn leave_topic(client: &mut Client, topic: &gossipsub::IdentTopic) -> eyre::Result<()> {
    client.unsubscribe(topic.clone()).await?;

    Ok(())
}

fn store_transaction(
    transaction: Transaction,
    storage: &Storage,
    sender: Option<PeerId>,
) -> eyre::Result<Hash> {
    let transaction_hash = hash(&transaction)?;
    let mut transactions_mutex = storage
        .transactions
        .lock()
        .map_err(|guard| eyre::eyre!("{:?}", guard))?;
    transactions_mutex.insert(transaction_hash.clone(), transaction);
    if let Some(peer_id) = sender {
        storage
            .senders
            .lock()
            .map_err(|guard| eyre::eyre!("{:?}", guard))?
            .insert(transaction_hash.clone(), peer_id);
    }

    Ok(transaction_hash)
}

async fn choose_coordinator(
    mut client: Client,
    storage: &mut Storage,
    application_topic_hash: gossipsub::TopicHash,
    coordinator_topic: gossipsub::IdentTopic,
) -> eyre::Result<()> {
    let mut chosen_coordinator = storage
        .chosen_coordinator
        .lock()
        .map_err(|guard| eyre::eyre!("{:?}", guard))?;
    if chosen_coordinator.is_none()
        || chosen_coordinator.is_chosen()
            && !client
                .mesh_peer_online(application_topic_hash, chosen_coordinator.to_id())
                .await
    {
        *chosen_coordinator = CoordinatorState::None;

        if storage.node_type.is_leader()
            && SystemTime::now()
                .duration_since(storage.last_network_event_timestamp)?
                .as_secs()
                > 0
        {
            if client.mesh_peer_count(coordinator_topic.hash()).await != 0 {
                info!("REQUESTING COORDINATOR");
                client
                    .publish(
                        coordinator_topic.hash(),
                        serde_json::to_vec(&CeremonyAction::RequestForCoordinator)?,
                    )
                    .await?;
                storage.last_network_event_timestamp = SystemTime::now();
            }
        }
    }
    Ok(())
}

async fn coordinator_event_recipient(
    mut client: Client,
    application_topic: gossipsub::IdentTopic,
    event: Event,
    storage: &mut Storage,
    coordinator_topic_hash: gossipsub::TopicHash,
) -> eyre::Result<()> {
    match event {
        Event::Subscribed {
            peer_id: their_peer_id,
            topic: topic_hash,
        } => {
            if coordinator_topic_hash == topic_hash {
                println!("info: {} joined the coordinators.", their_peer_id.cyan());
            }
        }
        Event::Message { message, .. } => {
            let source = message.source;
            if message.topic == application_topic.hash() {
                let message: NetworkAction = serde_json::from_slice(&message.data)?;

                match message {
                    NetworkAction::Transaction(transaction) => {
                        let transaction_hash = store_transaction(transaction, storage, source)?;

                        storage.nonce += 1;
                        let confirmation =
                            NetworkAction::TransactionConfirmation(TransactionConfirmation {
                                nonce: storage.nonce,
                                transaction_hash: transaction_hash.clone(),
                                // TODO proper confirmation hash
                                confirmation_hash: transaction_hash,
                            });
                        client
                            .publish(application_topic.hash(), serde_json::to_vec(&confirmation)?)
                            .await?;
                    }
                    NetworkAction::WhoIsCoordinator | NetworkAction::ChosenCoordinator(_) => {}
                    _ => error!("UNKNOWN message"),
                }
            } else if message.topic == coordinator_topic_hash {
                let message: CeremonyAction = serde_json::from_slice(&message.data)?;

                match message {
                    CeremonyAction::RequestForCoordinator => {
                        client
                            .publish(
                                coordinator_topic_hash,
                                serde_json::to_vec(&CeremonyAction::CoordinatorOffer)?,
                            )
                            .await?;
                    }
                    CeremonyAction::AcceptCoordinator(coordinator_id) => {
                        if coordinator_id == storage.peer_id {
                            info!("I AM ACCEPTED");
                            join_topic(&mut client, &application_topic).await?;
                            client
                                .publish(
                                    coordinator_topic_hash,
                                    serde_json::to_vec(&CeremonyAction::CoordinatorConfirm)?,
                                )
                                .await?;
                        }
                    }
                    _ => (),
                }
            }
        }
    }

    Ok(())
}

async fn peer_event_recipient(
    mut client: Client,
    application_topic_hash: gossipsub::TopicHash,
    event: Event,
    storage: &mut Storage,
    coordinator_topic: gossipsub::IdentTopic,
) -> eyre::Result<()> {
    let mut chosen_coordinator = storage
        .chosen_coordinator
        .lock()
        .map_err(|guard| eyre::eyre!("{:?}", guard))?;
    match event {
        Event::Subscribed {
            peer_id: their_peer_id,
            topic: topic_hash,
        } => {
            if application_topic_hash == topic_hash {
                println!(
                    "info: {} joined the application network.",
                    their_peer_id.cyan()
                );
            }
        }
        Event::Message { message, .. } => {
            let source = message.source;
            if message.topic == application_topic_hash {
                let message: NetworkAction = serde_json::from_slice(&message.data)?;

                match message {
                    NetworkAction::WhoIsCoordinator => {
                        if chosen_coordinator.is_chosen() {
                            info!("SENDING COORDINATOR INFO");
                            client
                                .publish(
                                    application_topic_hash,
                                    serde_json::to_vec(&NetworkAction::ChosenCoordinator(
                                        chosen_coordinator.to_id(),
                                    ))?,
                                )
                                .await?;
                        }
                    }
                    NetworkAction::ChosenCoordinator(coordinator_id) => {
                        info!("ACCEPTING COORDINATOR INFO");
                        *chosen_coordinator = CoordinatorState::Chosen(coordinator_id);
                    }
                    NetworkAction::Transaction(transaction) => {
                        store_transaction(transaction, storage, source)?;
                    }
                    NetworkAction::TransactionConfirmation(confirmation) => {
                        if source != chosen_coordinator.to_option() {
                            info!("Ignoring confirmation from wrong coordinator");
                        } else {
                            info!(
                                "Confirmation -> nonce: {}, transaction_hash: {:02X?}",
                                confirmation.nonce, confirmation.transaction_hash
                            );
                            let src = if let Some(peer_id) = storage
                                .senders
                                .lock()
                                .map_err(|guard| eyre::eyre!("{:?}", guard))?
                                .get(&confirmation.transaction_hash)
                            {
                                peer_id.green().to_string()
                            } else {
                                "<unknown>".to_owned()
                            };
                            println!(
                                "{}: {}",
                                src,
                                if let Some(transaction) = storage
                                    .transactions
                                    .lock()
                                    .map_err(|guard| eyre::eyre!("{:?}", guard))?
                                    .get(&confirmation.transaction_hash)
                                {
                                    match std::str::from_utf8(&transaction.payload[..]) {
                                        Ok(s) => s,
                                        Err(_) => "<binary>",
                                    }
                                } else {
                                    "<unknown>"
                                }
                            );
                        }
                    }
                    _ => error!("UNKNOWN"),
                }
            } else if message.topic == coordinator_topic.hash() {
                let message: CeremonyAction = serde_json::from_slice(&message.data)?;

                match message {
                    CeremonyAction::CoordinatorOffer => {
                        if storage.node_type.is_leader() {
                            info!("OFFER FROM: {:?}", source);
                            if chosen_coordinator.is_none() {
                                info!("ACCEPTING {:?}", source);
                                client
                                    .publish(
                                        coordinator_topic.hash(),
                                        serde_json::to_vec(&CeremonyAction::AcceptCoordinator(
                                            source.expect("coordinator has no address"),
                                        ))?,
                                    )
                                    .await?;
                                *chosen_coordinator =
                                    CoordinatorState::Pending(source.expect("From nowhere"));
                            }
                        }
                    }
                    CeremonyAction::CoordinatorConfirm => {
                        info!("CONFIRMATION FROM: {:?}", source);
                        let coordinator_id = source.expect("from nowhere");
                        if coordinator_id == chosen_coordinator.to_id() {
                            *chosen_coordinator = CoordinatorState::Chosen(coordinator_id);

                            client
                                .publish(
                                    application_topic_hash,
                                    serde_json::to_vec(&NetworkAction::ChosenCoordinator(
                                        chosen_coordinator.to_id(),
                                    ))?,
                                )
                                .await?;
                        }
                    }
                    _ => error!("UNKNOWN "),
                }
            }
        }
    }

    Ok(())
}

fn create_transaction(message: &str, storage: &Storage) -> eyre::Result<NetworkAction> {
    let transaction = Transaction {
        method: String::from("send_message"),
        payload: message.as_bytes().to_vec(),
        last_known_transaction_hash: storage.last_known_transaction_hash.lock().unwrap().clone(),
    };
    store_transaction(transaction.clone(), storage, Some(storage.peer_id))?;

    Ok(NetworkAction::Transaction(transaction))
}

async fn init(
    peer_id: PeerId,
    config: &Config,
) -> eyre::Result<(Client, mpsc::Receiver<Event>, EventLoop)> {
    let bootstrap_peers = {
        let mut peers = vec![];

        for mut addr in config.bootstrap.nodes.clone() {
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
        .with_behaviour(|key| Behaviour {
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
                let mut kad = kad::Behaviour::new(peer_id, kad::store::MemoryStore::new(peer_id));
                kad.set_mode(Some(kad::Mode::Server));
                for (peer_id, addr) in bootstrap_peers {
                    kad.add_address(&peer_id, addr);
                }
                if let Err(err) = kad.bootstrap() {
                    warn!("Failed to bootstrap with Kademlia: {}", err);
                }
                kad
            },
            gossipsub: gossipsub::Behaviour::new(
                gossipsub::MessageAuthenticity::Signed(key.clone()),
                gossipsub::Config::default(),
            )
            .expect("Valid gossipsub config."),
            relay: relay::Behaviour::new(peer_id, relay::Config::default()),
            ping: ping::Behaviour::default(),
        })?
        .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(time::Duration::from_secs(30)))
        .build();

    let (command_sender, command_receiver) = mpsc::channel(32);
    let (event_sender, event_receiver) = mpsc::channel(32);

    let client = Client {
        sender: command_sender,
    };

    let event_loop = EventLoop::new(swarm, command_receiver, event_sender);

    Ok((client, event_receiver, event_loop))
}

#[derive(Clone)]
pub(crate) struct Client {
    sender: mpsc::Sender<Command>,
}

impl Client {
    pub(crate) async fn listen_on(&mut self, addr: Multiaddr) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::ListenOn { addr, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub(crate) async fn bootstrap(&mut self) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Bootstrap { sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")?;

        Ok(())
    }

    pub(crate) async fn subscribe(
        &mut self,
        topic: gossipsub::IdentTopic,
    ) -> eyre::Result<gossipsub::IdentTopic> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Subscribe { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub(crate) async fn unsubscribe(
        &mut self,
        topic: gossipsub::IdentTopic,
    ) -> eyre::Result<gossipsub::IdentTopic> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Unsubscribe { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub(crate) async fn mesh_peer_count(&mut self, topic: gossipsub::TopicHash) -> usize {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::MeshPeerCount { topic, sender })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
    }

    pub(crate) async fn mesh_peer_online(
        &mut self,
        topic: gossipsub::TopicHash,
        peer_id: PeerId,
    ) -> bool {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::MeshPeerOnline {
                topic,
                sender,
                peer_id,
            })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.") != 0
    }

    pub(crate) async fn publish(
        &mut self,
        topic: gossipsub::TopicHash,
        data: Vec<u8>,
    ) -> eyre::Result<gossipsub::MessageId> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Command::Publish {
                topic,
                data,
                sender,
            })
            .await
            .expect("Command receiver not to be dropped.");

        receiver.await.expect("Sender not to be dropped.")
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
        let mut interval = time::interval(time::Duration::from_secs(5));
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
            Command::MeshPeerCount { topic, sender } => {
                let _ = sender.send(
                    self.swarm
                        .behaviour_mut()
                        .gossipsub
                        .mesh_peers(&topic)
                        .count(),
                );
            }
            Command::MeshPeerOnline {
                topic,
                sender,
                peer_id,
            } => {
                let _ = sender.send(
                    self.swarm
                        .behaviour_mut()
                        .gossipsub
                        .mesh_peers(&topic)
                        .any(|p| p == &peer_id) as usize,
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
    MeshPeerCount {
        topic: gossipsub::TopicHash,
        sender: oneshot::Sender<usize>,
    },
    MeshPeerOnline {
        topic: gossipsub::TopicHash,
        sender: oneshot::Sender<usize>,
        peer_id: PeerId,
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

#[derive(Debug)]
pub(crate) enum Event {
    Subscribed {
        peer_id: PeerId,
        topic: gossipsub::TopicHash,
    },
    Message {
        id: gossipsub::MessageId,
        message: gossipsub::Message,
    },
}

type Hash = Vec<u8>;

fn hash<T: Serialize>(item: &T) -> eyre::Result<Hash> {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&item)?);
    Ok(hasher.finalize().to_vec())
}

type Signature = Vec<u8>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Transaction {
    pub method: String,
    pub payload: Vec<u8>,
    pub last_known_transaction_hash: Hash,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TransactionConfirmation {
    pub nonce: u64,
    pub transaction_hash: Hash,
    // sha256(previous_confirmation_hash, transaction_hash, nonce)
    pub confirmation_hash: Hash,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CatchupRequest {
    pub last_executed_transaction_hash: Hash,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct TransactionWithConfirmation {
    pub transaction: Transaction,
    pub confirmation: TransactionConfirmation,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CatchupResponse {
    pub transactions: Vec<TransactionWithConfirmation>,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum NetworkAction {
    Transaction(Transaction),
    TransactionConfirmation(TransactionConfirmation),
    CatchupRequest(CatchupRequest),
    CatchupResponse(CatchupResponse),
    WhoIsCoordinator,
    ChosenCoordinator(PeerId),
}

#[derive(Serialize, Deserialize)]
pub enum CeremonyAction {
    RequestForCoordinator,
    CoordinatorOffer,
    AcceptCoordinator(PeerId),
    CoordinatorConfirm,
}

#[derive(Serialize, Deserialize)]
pub struct SignedNetworkAction {
    pub action: NetworkAction,
    pub signature: Signature,
}

#[derive(Serialize, Deserialize)]
pub enum CoordinatorState {
    None,
    Pending(PeerId),
    Chosen(PeerId),
}

impl CoordinatorState {
    pub fn is_none(&self) -> bool {
        match *self {
            CoordinatorState::None => true,
            _ => false,
        }
    }

    pub fn is_pending(&self) -> bool {
        match *self {
            CoordinatorState::Pending(_) => true,
            _ => false,
        }
    }

    pub fn is_chosen(&self) -> bool {
        match *self {
            CoordinatorState::Chosen(_) => true,
            _ => false,
        }
    }

    pub fn to_option(&self) -> Option<PeerId> {
        match *self {
            CoordinatorState::None => None,
            CoordinatorState::Pending(x) => Some(x),
            CoordinatorState::Chosen(x) => Some(x),
        }
    }

    pub fn to_id(&self) -> PeerId {
        match *self {
            CoordinatorState::None => panic!("No chosen coordinator"),
            CoordinatorState::Pending(x) => x,
            CoordinatorState::Chosen(x) => x,
        }
    }
}
