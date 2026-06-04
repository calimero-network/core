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
use multiaddr::{Multiaddr, Protocol};
use prometheus_client::registry::Registry;
use tokio::sync::oneshot;
use tokio::time::interval;
use tokio_stream::wrappers::IntervalStream;
use tracing::{error, info, warn};

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
        let mut swarm = Behaviour::build_swarm(config)?;

        // Seed operator-configured external addresses directly into the
        // swarm's confirmed set. Deterministic, no third-party lookup —
        // for known-static-IP / hosted deployments. When none are
        // configured, external addresses are discovered via identify +
        // AutoNAT v2 confirmation instead (see the identify handler).
        //
        // These bypass AutoNAT dial-back confirmation, so a typo or a
        // stale entry would otherwise be advertised to every peer as
        // reachable. Skip addresses that can never be reached by a remote
        // peer (loopback / unspecified / link-local) rather than poison
        // the swarm's external set with them; warn on private/ULA ranges,
        // which are legitimate for same-network deployments but a common
        // misconfiguration when a node is meant to be publicly reachable.
        if config.discovery.advertise_address {
            for addr in &config.discovery.external_address {
                if is_seedable_external_address(addr) {
                    info!(%addr, "Seeding operator-configured external address");
                    swarm.add_external_address(addr.clone());
                } else {
                    warn!(
                        %addr,
                        "Ignoring non-routable external_address (loopback / unspecified / \
                         link-local): remote peers cannot dial it",
                    );
                }
            }
        }

        let discovery = Discovery::new(
            &config.discovery.rendezvous,
            &config.discovery.relay,
            &config.discovery.autonat,
            reserved_topics,
        );

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

/// Whether an operator-configured external address is worth seeding into
/// the swarm's confirmed external-address set.
///
/// Configured addresses skip AutoNAT dial-back confirmation, so we filter
/// out the ones a remote peer can provably never reach — loopback,
/// unspecified (`0.0.0.0` / `::`), and link-local — instead of advertising
/// nonsense. Private (RFC-1918) and IPv6 unique-local (`fc00::/7`)
/// addresses are kept, since they're valid for same-network / overlay
/// deployments, but warned about because they're a frequent slip when the
/// node is actually meant to be publicly reachable. Addresses with no IP
/// component (e.g. a DNS multiaddr) are passed through unchanged.
fn is_seedable_external_address(addr: &Multiaddr) -> bool {
    for proto in addr.iter() {
        match proto {
            Protocol::Ip4(ip) => {
                if ip.is_loopback() || ip.is_unspecified() || ip.is_link_local() {
                    return false;
                }
                if ip.is_private() {
                    warn!(%addr, "external_address is a private (RFC-1918) range; only reachable within the same network");
                }
            }
            Protocol::Ip6(ip) => {
                if ip.is_loopback() || ip.is_unspecified() {
                    return false;
                }
                // Link-local fe80::/10 — never routable off-link.
                if (ip.segments()[0] & 0xffc0) == 0xfe80 {
                    return false;
                }
                // Unique-local fc00::/7 — site-scoped, not globally reachable.
                if (ip.segments()[0] & 0xfe00) == 0xfc00 {
                    warn!(%addr, "external_address is an IPv6 unique-local (fc00::/7) range; only reachable within the same network");
                }
            }
            _ => {}
        }
    }
    true
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

#[cfg(test)]
mod external_address_tests {
    use super::is_seedable_external_address;

    fn seedable(s: &str) -> bool {
        is_seedable_external_address(&s.parse().expect("valid multiaddr"))
    }

    #[test]
    fn routable_addresses_are_seedable() {
        assert!(seedable("/ip4/203.0.113.7/tcp/2428"));
        assert!(seedable("/ip6/2001:db8::1/tcp/2428"));
        // Private / unique-local are kept (warned), valid for overlays.
        assert!(seedable("/ip4/10.0.0.5/tcp/2428"));
        assert!(seedable("/ip4/192.168.1.20/udp/2428/quic-v1"));
        assert!(seedable("/ip6/fd00::1/tcp/2428"));
        // No IP component (DNS) is passed through.
        assert!(seedable("/dns4/node.example.com/tcp/2428"));
    }

    #[test]
    fn non_routable_addresses_are_skipped() {
        assert!(!seedable("/ip4/127.0.0.1/tcp/2428"));
        assert!(!seedable("/ip4/0.0.0.0/tcp/2428"));
        assert!(!seedable("/ip4/169.254.1.1/tcp/2428"));
        assert!(!seedable("/ip6/::1/tcp/2428"));
        assert!(!seedable("/ip6/::/tcp/2428"));
        assert!(!seedable("/ip6/fe80::1/tcp/2428"));
    }
}
