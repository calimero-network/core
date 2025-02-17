#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]
use std::collections::hash_map::HashMap;
use std::ops::DerefMut;
use std::ptr;

use actix::{Actor, Addr, AsyncContext, Context, Running};
use calimero_utils_actix::spawn_actor;
use client::NetworkClient;
use config::NetworkConfig;
use eyre::{bail, Result as EyreResult};
use futures_util::future::OptionFuture;
use futures_util::StreamExt;
use handler::stream::FromStreamInner;
use libp2p::dcutr::Behaviour as DcutrBehaviour;
use libp2p::gossipsub::{
    Behaviour as GossipsubBehaviour, Config as GossipsubConfig, MessageAuthenticity,
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
use libp2p::swarm::{NetworkBehaviour, Swarm};
use libp2p::tcp::Config as TcpConfig;
use libp2p::tls::Config as TlsConfig;
use libp2p::yamux::Config as YamuxConfig;
use libp2p::{PeerId, StreamProtocol, SwarmBuilder};
use libp2p_stream::{Behaviour as StreamBehaviour, IncomingStreams};
use mock::NodeManagerMock;
use multiaddr::Protocol;
use stream::CALIMERO_STREAM_PROTOCOL;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{interval, Duration};
use tokio::{select, spawn};
use tracing::warn;

use crate::discovery::Discovery;
use crate::types::NetworkEvent;

pub mod client;
pub mod config;
mod discovery;
mod handler;
pub mod stream;
pub mod types;

mod mock;

const PROTOCOL_VERSION: &str = concat!("/", env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const CALIMERO_KAD_PROTO_NAME: StreamProtocol = StreamProtocol::new("/calimero/kad/1.0.0");

#[derive(NetworkBehaviour)]
struct Behaviour {
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

    let (client, event_receiver, event_loop) = init(peer_id, config)?;

    drop(spawn(event_loop.run()));

    for addr in &config.swarm.listen {
        client.listen_on(addr.clone()).await?;
    }

    drop(client.bootstrap().await);

    Ok((client, event_receiver))
}

fn init(
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
        .accept(CALIMERO_STREAM_PROTOCOL)
    {
        Ok(incoming_streams) => incoming_streams,
        Err(err) => {
            bail!("Failed to setup control for stream protocol: {:?}", err)
        }
    };

    let (command_sender, command_receiver) = mpsc::channel(32);
    let (event_sender, event_receiver) = mpsc::channel(32);

    let client = NetworkClient {
        _sender: command_sender,
    };

    let discovery = Discovery::new(&config.discovery.rendezvous, &config.discovery.relay);

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
    swarm: Box<Swarm<Behaviour>>,
    incoming_streams: Box<IncomingStreams>,
    command_receiver: mpsc::Receiver<Command>,
    event_sender: mpsc::Sender<NetworkEvent>,
    node_manager: Addr<NodeManagerMock>,
    discovery: Discovery,
    pending_dial: HashMap<PeerId, oneshot::Sender<EyreResult<Option<()>>>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<EyreResult<Option<()>>>>,
}

#[allow(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
impl EventLoop {
    fn new(
        swarm: Swarm<Behaviour>,
        incoming_streams: IncomingStreams,
        command_receiver: mpsc::Receiver<Command>,
        event_sender: mpsc::Sender<NetworkEvent>,
        discovery: Discovery,
    ) -> Self {
        Self {
            swarm: Box::new(swarm),
            incoming_streams: Box::new(incoming_streams),
            command_receiver,
            event_sender,
            node_manager: NodeManagerMock::start_default(),
            discovery,
            pending_dial: HashMap::default(),
            pending_bootstrap: HashMap::default(),
        }
    }

    pub(crate) async fn run(mut self) {
        let mut rendezvous_discover_tick =
            interval(self.discovery.rendezvous_config.discovery_interval);

        #[expect(clippy::redundant_pub_crate, reason = "Needed for Tokio code")]
        loop {
            select! {
                // event = self.swarm.next() => {
                //     self.handle_swarm_event(event.expect("Swarm stream to be infinite."));
                // },
                // incoming_stream = self.incoming_streams.next() => {
                //     self.handle_incoming_stream(incoming_stream.expect("Incoming streams to be infinite."));
                // },
                // command = self.command_receiver.recv() => {
                //     let Some(c) = command else { break };
                //     self.handle_command(c).await;
                // }
                _ = rendezvous_discover_tick.tick() => self.broadcast_rendezvous_discoveries(),
            }
        }
    }
}

#[derive(Debug)]
struct Command;

impl Actor for EventLoop {
    type Context = Context<Self>;

    fn start(mut self) -> Addr<Self>
    where
        Self: Actor<Context = Context<Self>>,
    {
        spawn_actor!(self @ EventLoop => {
            .swarm as FromSwarm
        })
    }

    fn stopping(&mut self, _ctx: &mut Self::Context) -> Running {
        println!("stopping the network manager");
        Running::Stop
    }
}
