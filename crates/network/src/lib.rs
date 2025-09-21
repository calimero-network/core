#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]
#![expect(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
use std::collections::hash_map::HashMap;

use actix::{Actor, AsyncContext, Context};
use calimero_network_primitives::config::NetworkConfig;
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::stream::{CALIMERO_BLOB_PROTOCOL, CALIMERO_STREAM_PROTOCOL};
use calimero_utils_actix::{actor, LazyRecipient};
use eyre::Result as EyreResult;
use futures_util::StreamExt;
use libp2p::kad::QueryId;
use libp2p::swarm::Swarm;
use libp2p::PeerId;
use libp2p_metrics::Metrics;
use prometheus_client::registry::Registry;
use tokio::sync::oneshot;
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tracing::error;

use crate::handlers::stream::incoming::FromIncoming;

mod behaviour;
mod discovery;
mod handlers;
mod store;

use behaviour::Behaviour;
use discovery::Discovery;
use handlers::stream::rendezvous::RendezvousTick;
use handlers::stream::swarm::FromSwarm;

#[expect(
    missing_debug_implementations,
    reason = "Swarm doesn't implement Debug"
)]
pub struct NetworkManager {
    swarm: Box<Swarm<Behaviour>>,
    event_recipient: LazyRecipient<NetworkEvent>,
    discovery: Discovery,
    pending_dial: HashMap<PeerId, oneshot::Sender<EyreResult<()>>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<EyreResult<()>>>,
    pending_blob_queries: HashMap<QueryId, oneshot::Sender<eyre::Result<Vec<PeerId>>>>,
    metrics: Metrics,
}

impl NetworkManager {
    pub async fn new(
        config: &NetworkConfig,
        event_recipient: LazyRecipient<NetworkEvent>,
        prom_registry: &mut Registry,
        db: calimero_store::Store,
    ) -> eyre::Result<Self> {
        let swarm = Behaviour::build_swarm(config, db)?;

        let discovery = Discovery::new(
            &config.discovery.rendezvous,
            &config.discovery.relay,
            &config.discovery.autonat,
            config
                .discovery
                .advertise_address
                .then_some(&*config.swarm.listen)
                .unwrap_or(&[]),
        )
        .await?;

        let this = Self {
            swarm: Box::new(swarm),
            event_recipient,
            discovery,
            pending_dial: HashMap::default(),
            pending_bootstrap: HashMap::default(),
            pending_blob_queries: HashMap::new(),
            metrics: Metrics::new(prom_registry),
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
                    ctx.add_stream(incoming_streams.map(|(peer_id, stream)| {
                        FromIncoming::from_stream(peer_id, stream, CALIMERO_STREAM_PROTOCOL)
                    }));
            }
            Err(err) => {
                error!("Failed to setup control for stream protocol: {:?}", err);
            }
        };

        match control.accept(CALIMERO_BLOB_PROTOCOL) {
            Ok(incoming_blob_streams) => {
                let _incoming_blob_streams_handle =
                    ctx.add_stream(incoming_blob_streams.map(|(peer_id, stream)| {
                        FromIncoming::from_stream(peer_id, stream, CALIMERO_BLOB_PROTOCOL)
                    }));
            }
            Err(err) => {
                error!("Failed to setup control for blob protocol: {:?}", err);
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
