#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]
#![expect(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
use std::collections::hash_map::HashMap;
use std::collections::BTreeSet;
use std::sync::Arc;

use calimero_store::Store;

use actix::{Actor, AsyncContext, Context};
use calimero_network_primitives::config::NetworkConfig;
use calimero_network_primitives::messages::NetworkEventDispatcher;
use calimero_network_primitives::stream::{CALIMERO_BLOB_PROTOCOL, CALIMERO_STREAM_PROTOCOL};
use calimero_utils_actix::actor;
use eyre::Result as EyreResult;
use futures_util::StreamExt;
use libp2p::kad::QueryId;
use libp2p::swarm::{ConnectionId, Swarm};
use libp2p::PeerId;
use libp2p_metrics::Metrics;
use prometheus_client::registry::Registry;
use tokio::sync::oneshot;
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tracing::error;

use crate::handlers::stream::incoming::FromIncoming;

pub use calimero_network_primitives::autonat_v2 as autonat;
pub mod behaviour;
mod discovery;
mod handlers;

use behaviour::Behaviour;
use discovery::peer_cache::PeerAddrCache;
use discovery::Discovery;
use handlers::stream::rendezvous::RendezvousTick;
use handlers::stream::swarm::FromSwarm;

#[expect(
    missing_debug_implementations,
    reason = "Swarm doesn't implement Debug"
)]
pub struct NetworkManager {
    swarm: Box<Swarm<Behaviour>>,
    event_dispatcher: Arc<dyn NetworkEventDispatcher>,
    discovery: Discovery,
    /// Persistent cache of addresses for peers that share our overlays,
    /// for fast reconnect on restart. Recorded on connect, loaded+dialed
    /// on startup, re-persisted on the rendezvous tick. See the
    /// `peer_cache_*` methods on the discovery impl.
    peer_cache: PeerAddrCache,
    /// Datastore handle the peer cache is persisted to (a node-local
    /// blob under a `Generic` key — the datastore-backed peerstore
    /// pattern). `None` disables persistence (tests / no store).
    store: Option<Store>,
    pending_dial: HashMap<PeerId, oneshot::Sender<EyreResult<()>>>,
    pending_bootstrap: HashMap<QueryId, oneshot::Sender<EyreResult<()>>>,
    pending_blob_queries: HashMap<QueryId, oneshot::Sender<eyre::Result<Vec<PeerId>>>>,
    // Consecutive ping failures per live connection. A silent network
    // partition (no TCP FIN/RST — e.g. a cable pull, a Wi-Fi drop, or a
    // Docker `network disconnect`) leaves a connection wedged "open" from
    // libp2p's view: the kernel socket never errors, so no `ConnectionClosed`
    // ever fires, and every recovery path keyed on `ConnectionClosed`
    // (relay-reservation re-acquisition, regular-peer force-rediscovery)
    // stays dormant while the peer is unreachable. The ping behaviour is the
    // only subsystem that actively probes liveness, so we lean on it: count
    // consecutive ping failures per connection and, once a connection trips
    // `MAX_PING_FAILURES`, close it ourselves to synthesise the
    // `ConnectionClosed` that the rest of the recovery machinery waits for.
    // Reset to absent on the next ping success or when the connection closes.
    ping_failures: HashMap<ConnectionId, u32>,
    metrics: Metrics,
}

impl NetworkManager {
    /// Create a new NetworkManager with an event dispatcher.
    ///
    /// The dispatcher receives all network events (gossipsub messages, streams, etc.)
    /// and must implement `NetworkEventDispatcher` for reliable delivery.
    pub async fn new(
        config: &NetworkConfig,
        event_dispatcher: Arc<dyn NetworkEventDispatcher>,
        prom_registry: &mut Registry,
        reserved_topics: BTreeSet<String>,
        store: Option<Store>,
    ) -> eyre::Result<Self> {
        let swarm = Behaviour::build_swarm(config)?;

        let discovery = Discovery::new(
            &config.discovery.rendezvous,
            &config.discovery.relay,
            &config.discovery.autonat,
            if config.discovery.advertise_address {
                &config.swarm.listen
            } else {
                &[]
            },
            reserved_topics,
        )
        .await?;

        let this = Self {
            swarm: Box::new(swarm),
            event_dispatcher,
            discovery,
            peer_cache: PeerAddrCache::default(),
            store,
            pending_dial: HashMap::default(),
            pending_bootstrap: HashMap::default(),
            pending_blob_queries: HashMap::new(),
            ping_failures: HashMap::default(),
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
        }

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
        }

        let _ping_handle = ctx.add_stream(
            IntervalStream::new(interval(
                self.discovery.rendezvous_config.discovery_interval,
            ))
            .map(RendezvousTick::from),
        );

        // Fast reconnect: dial peers we cached from a previous run before
        // rendezvous rediscovery has a chance to run. Best-effort and
        // deduped at the swarm level; stale entries fail and age out.
        self.load_peer_cache_and_dial();
    }
}
