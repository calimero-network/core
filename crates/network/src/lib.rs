#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]
#[expect(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
use std::collections::hash_map::HashMap;

use actix::{Actor, AsyncContext, Context};
use calimero_utils_actix::{actor, LazyAddr, LazyRecipient};
use eyre::Result as EyreResult;
use futures_util::StreamExt;
use libp2p::kad::QueryId;
use libp2p::swarm::Swarm;
use libp2p::PeerId;
use tokio::sync::{mpsc, oneshot};
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tracing::error;

mod behaviour;
pub mod client;
pub mod config;
mod discovery;
mod handler;
pub mod stream;
pub mod types;

use behaviour::Behaviour;
use client::NetworkClient;
use config::NetworkConfig;
use discovery::Discovery;
use handler::stream::incoming::FromIncoming;
use handler::stream::rendezvous::RendezvousTick;
use handler::stream::swarm::FromSwarm;
use stream::CALIMERO_STREAM_PROTOCOL;
use types::NetworkEvent;

pub async fn run(
    config: &NetworkConfig,
) -> EyreResult<(NetworkClient, mpsc::Receiver<NetworkEvent>)> {
    let network_manager = NetworkManager::new(config, LazyRecipient::new())?;

    let network_manager_addr = LazyAddr::new();

    let client = NetworkClient::new(network_manager_addr.clone());

    let _ignored = network_manager_addr
        .init(|pending| {
            Actor::create(|ctx| {
                pending.process(ctx);
                network_manager
            })
        })
        .await
        .expect("should not already be initialized");

    client.bootstrap().await?;

    let (_event_sender, event_receiver) = mpsc::channel(32);

    Ok((client, event_receiver))
}

#[expect(
    missing_debug_implementations,
    reason = "Swarm doesn't implement Debug"
)]
pub struct NetworkManager {
    swarm: Box<Swarm<Behaviour>>,
    event_recipient: LazyRecipient<NetworkEvent>,
    discovery: Discovery,
    pending_dial: HashMap<PeerId, oneshot::Sender<EyreResult<Option<()>>>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<EyreResult<Option<()>>>>,
}

impl NetworkManager {
    pub fn new(
        config: &NetworkConfig,
        event_recipient: LazyRecipient<NetworkEvent>,
    ) -> eyre::Result<Self> {
        let swarm = Behaviour::build_swarm(config)?;

        let this = Self {
            swarm: Box::new(swarm),
            event_recipient,
            discovery: Discovery::new(&config.discovery.rendezvous, &config.discovery.relay),
            pending_dial: HashMap::default(),
            pending_bootstrap: HashMap::default(),
        };

        Ok(this)
    }
}

impl Actor for NetworkManager {
    type Context = Context<Self>;

    actor!(NetworkManager => {
        .swarm as FromSwarm
    });

    fn started(&mut self, ctx: &mut Context<Self>) {
        let mut control = self.swarm.behaviour().stream.new_control();

        match control.accept(CALIMERO_STREAM_PROTOCOL) {
            Ok(incoming_streams) => {
                let _inoming_streams_handle =
                    ctx.add_stream(incoming_streams.map(FromIncoming::from));
            }
            Err(err) => {
                error!("Failed to setup control for stream protocol: {:?}", err);
            }
        };

        let _ping_handle = ctx.add_stream(
            IntervalStream::new(interval(
                self.discovery.rendezvous_config.discovery_interval,
            ))
            .map(RendezvousTick::from),
        );
    }
}
