use core::net::Ipv4Addr;
use core::time::Duration;
use std::collections::{BTreeSet, HashMap, HashSet};

use calimero_network_primitives::config::{AutonatConfig, RelayConfig, RendezvousConfig};
use eyre::{bail, ContextCompat, Result as EyreResult, WrapErr as _};
use libp2p::rendezvous::client::RegisterError;
use libp2p::rendezvous::Namespace;
use libp2p::PeerId;
use multiaddr::{Multiaddr, Protocol};
use tracing::{debug, error};

use super::NetworkManager;
use crate::discovery::state::{
    rendezvous_key_for_topic, under_connected_rendezvous_keys, DiscoveryState, ReachabilityActions,
    RelayReservationStatus, RendezvousRegistrationStatus,
};

pub mod state;

// Persistent relevant-peer address cache. Methods are exercised by the
// module's own unit tests and wired into the swarm/command layer in the
// next slice (record-on-connect, export/import commands, node-side file
// persistence); allow dead_code until that lands so the foundation can
// be reviewed independently.
#[allow(dead_code)]
pub(crate) mod peer_cache;

/// Max rendezvous `discover` requests issued in a single
/// `rendezvous_discover` call. Bounds the per-tick discovery fan-out so a
/// node that belongs to many under-connected overlays doesn't flood the
/// rendezvous server in one pass; the rotating
/// [`Discovery::rendezvous_discover_cursor`] ensures every under-connected
/// key is eventually covered across successive ticks.
const RENDEZVOUS_DISCOVER_BUDGET: usize = 8;

#[derive(Debug)]
pub struct Discovery {
    pub(crate) state: DiscoveryState,
    pub(crate) rendezvous_config: RendezvousConfig,
    pub(crate) relay_config: RelayConfig,
    pub(crate) advertise: Option<AdvertiseState>,
    pub(crate) _autonat_config: AutonatConfig,
    /// Rotating offset into the under-connected rendezvous-key list, so
    /// successive `rendezvous_discover` calls cover different keys when
    /// there are more under-connected overlays than the per-call budget
    /// (`RENDEZVOUS_DISCOVER_BUDGET`). Without rotation, keys past the
    /// budget would be starved of discovery forever on a node that
    /// belongs to many namespaces/groups.
    pub(crate) rendezvous_discover_cursor: usize,
    /// Subscribed gossipsub topics that must NOT be mapped to a
    /// per-overlay rendezvous key — currently the specialized-node
    /// invite topic, which every node subscribes to. Registering all
    /// nodes under one invite-topic key would recreate the global
    /// fan-out we're eliminating. Every other subscribed topic
    /// (`ns/`, `group/`, or a bare context id) is treated as an overlay.
    pub(crate) reserved_topics: BTreeSet<String>,
}

#[derive(Debug)]
pub struct AdvertiseState {
    pub(crate) ip: Ipv4Addr,
    pub(crate) ports: HashSet<u16>,
}

impl Discovery {
    pub(crate) async fn new(
        rendezvous_config: &RendezvousConfig,
        relay_config: &RelayConfig,
        autonat_config: &AutonatConfig,
        listening_on: &[Multiaddr],
        reserved_topics: BTreeSet<String>,
    ) -> EyreResult<Self> {
        let advertise = if listening_on.is_empty() {
            None
        } else {
            let ports = listening_on
                .iter()
                .filter_map(|addr| {
                    addr.iter().find_map(|p| match p {
                        Protocol::Tcp(port) | Protocol::Udp(port) => Some(port),
                        _ => None,
                    })
                })
                .collect();

            Some(AdvertiseState {
                ip: Self::get_public_ip().await?,
                ports,
            })
        };

        let this = Self {
            state: DiscoveryState::default(),
            rendezvous_config: rendezvous_config.clone(),
            relay_config: relay_config.clone(),
            advertise,
            _autonat_config: autonat_config.clone(),
            rendezvous_discover_cursor: 0,
            reserved_topics,
        };

        Ok(this)
    }

    async fn get_public_ip() -> EyreResult<Ipv4Addr> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()?;

        let ip_addr = client
            .get("https://api.ipify.org")
            .send()
            .await?
            .text()
            .await?
            .parse()?;

        Ok(ip_addr)
    }
}

impl NetworkManager {
    /// Rendezvous keys for every overlay topic this node currently
    /// follows — `ns/<hex>`, `group/<hex>`, and bare context ids — minus
    /// the reserved (non-overlay) topics. Used for registration so
    /// co-members can find us under the exact namespace/group/context key.
    fn overlay_rendezvous_keys(&self) -> Vec<Namespace> {
        let reserved = &self.discovery.reserved_topics;
        self.swarm
            .behaviour()
            .gossipsub
            .topics()
            .filter(|topic| !reserved.contains(topic.as_str()))
            .filter_map(|topic| rendezvous_key_for_topic(topic.as_str()))
            .collect()
    }

    /// Rendezvous keys for overlay topics where we currently have **no
    /// connected mesh peer** — the demand-driven discovery set. A topic
    /// that already has a co-member in its mesh can sync through it, so
    /// re-discovering it would only add rendezvous load. This keeps
    /// discovery cost proportional to how many overlays are starved
    /// (≈0 in steady state; a burst only right after restart/partition).
    fn under_connected_overlay_keys(&self) -> Vec<Namespace> {
        let reserved = &self.discovery.reserved_topics;
        let gossipsub = &self.swarm.behaviour().gossipsub;
        // Count connected SUBSCRIBERS per topic (the full peer-topics
        // set), not grafted mesh peers. A topic with a connected
        // subscriber can already sync through it even if the mesh is
        // momentarily thin (GRAFT lag / churn), so it isn't
        // under-connected — counting the mesh instead would re-trigger
        // discovery on healthy-but-unmeshed topics.
        let mut subscriber_counts: HashMap<String, usize> = HashMap::new();
        for (_peer, topics) in gossipsub.all_peers() {
            for topic in topics {
                *subscriber_counts
                    .entry(topic.as_str().to_owned())
                    .or_default() += 1;
            }
        }
        let topics: Vec<(String, usize)> = gossipsub
            .topics()
            .filter(|topic| !reserved.contains(topic.as_str()))
            .map(|topic| {
                let count = subscriber_counts.get(topic.as_str()).copied().unwrap_or(0);
                (topic.as_str().to_owned(), count)
            })
            .collect();
        under_connected_rendezvous_keys(topics.iter().map(|(t, c)| (t.as_str(), *c)))
    }

    // Sends rendezvous discovery requests to the rendezvous peer, one per
    // overlay key we're under-connected on, so `discover` returns only
    // co-members of our namespaces/groups rather than the whole network.
    //
    // Throttled via `discovery_rpm` unless `force == true` — see the
    // `rendezvous_discover_force` field on `ReachabilityActions` for
    // when callers should bypass the throttle (event-driven recovery
    // paths where the throttle floor exceeds the recovery budget).
    // Bounded per call by `RENDEZVOUS_DISCOVER_BUDGET` with a rotating
    // cursor so a node in many namespaces covers every under-connected
    // key across ticks without flooding the server in one pass.
    //
    // This function expects that the rendezvous peer is already
    // connected.
    pub(crate) fn rendezvous_discover(
        &mut self,
        rendezvous_peer: &PeerId,
        force: bool,
    ) -> EyreResult<()> {
        let throttled = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err_with(|| format!("Failed to get peer info for {rendezvous_peer}"))?
            .is_rendezvous_discover_throttled(self.discovery.rendezvous_config.discovery_rpm);

        if !force && throttled {
            return Ok(());
        }

        let mut keys = self.under_connected_overlay_keys();
        if keys.is_empty() {
            return Ok(());
        }

        // Rotate so successive calls cover keys past the budget. Discover
        // with a fresh cookie (`None`) under each key — per-overlay sets
        // are small, so a full poll each time is cheap and avoids
        // per-(peer, namespace) cookie bookkeeping; the `Discovered`
        // handler dedups the returned peers before dialing.
        let cursor = self.discovery.rendezvous_discover_cursor % keys.len();
        keys.rotate_left(cursor);
        let budget = RENDEZVOUS_DISCOVER_BUDGET.min(keys.len());
        self.discovery.rendezvous_discover_cursor = self
            .discovery
            .rendezvous_discover_cursor
            .wrapping_add(budget);

        for key in keys.into_iter().take(budget) {
            self.swarm.behaviour_mut().rendezvous.discover(
                Some(key.clone()),
                None,
                None,
                *rendezvous_peer,
            );
            debug!(
                %rendezvous_peer,
                rendezvous_namespace = %key,
                force,
                "Sent per-overlay discover request to rendezvous node"
            );
        }

        Ok(())
    }

    // Sends rendezvous registrations request to all rendezvous peers which require registration.
    // If rendezvous peer is not connected, it will be dialed which will trigger the registration during identify exchange.
    pub(crate) fn broadcast_rendezvous_registrations(&mut self) {
        #[expect(clippy::needless_collect, reason = "Necessary here; false positive")]
        for peer_id in self
            .discovery
            .state
            .get_rendezvous_peer_ids()
            .collect::<Vec<_>>()
        {
            let Some(peer_info) = self.discovery.state.get_peer_info(&peer_id) else {
                error!(%peer_id, "Failed to lookup peer info");
                continue;
            };

            if !self.discovery.state.is_rendezvous_registration_required(
                self.discovery.rendezvous_config.registrations_limit,
            ) {
                continue;
            }

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial relay peer");
                    }
                }
            } else if let Err(err) = self.rendezvous_register(&peer_id) {
                error!(%err, "Failed to update rendezvous registration");
            }
        }
    }

    // Registers our external addresses with `rendezvous_peer` under one
    // key per overlay we belong to (`/calimero/ns/<hex>`,
    // `/calimero/grp/<hex>`), so co-members discovering under that exact
    // key find us — instead of one global namespace that returns every
    // node on the network. A node that belongs to no overlay yet (fresh,
    // pre-join) registers under nothing and simply isn't discoverable
    // until it joins something, which is correct: there's nothing to
    // discover it for.
    //
    // If there are no external addresses for the node, registration is
    // considered successful. Expects the rendezvous peer to be connected.
    pub(crate) fn rendezvous_register(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        // `registrations_limit` gates how many rendezvous *peers* we
        // register with (infra fan-out), independent of how many overlay
        // keys we register under with each.
        if !self.discovery.state.is_rendezvous_registration_required(
            self.discovery.rendezvous_config.registrations_limit,
        ) {
            return Ok(());
        }

        let keys = self.overlay_rendezvous_keys();
        if keys.is_empty() {
            debug!(
                %rendezvous_peer,
                "No overlay topics to register under yet; skipping rendezvous registration"
            );
            return Ok(());
        }

        let mut registered_any = false;
        for key in keys {
            match self.swarm.behaviour_mut().rendezvous.register(
                key.clone(),
                *rendezvous_peer,
                None,
            ) {
                Ok(()) => {
                    registered_any = true;
                    debug!(
                        %rendezvous_peer,
                        rendezvous_namespace = %key,
                        "Sent register request to rendezvous node"
                    );
                }
                Err(RegisterError::NoExternalAddresses) => {
                    // No external addresses yet — nothing to register
                    // anywhere this round; stop early, the next
                    // reachability/identify event retries.
                    debug!("No external addresses to register at rendezvous");
                    return Ok(());
                }
                Err(err @ RegisterError::FailedToMakeRecord(_)) => bail!(err),
            }
        }

        if registered_any {
            self.discovery.state.update_rendezvous_registration_status(
                rendezvous_peer,
                RendezvousRegistrationStatus::Requested,
            );
        }

        Ok(())
    }

    // We unregister from a rendezvous peer if we were previously registered.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_unregister(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        let status = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info")?
            .rendezvous()
            .wrap_err("Peer isn't rendezvous")?
            .registration_status();

        match status {
            RendezvousRegistrationStatus::Registered => {
                // Actively unregister from the rendezvous server under
                // every overlay key we registered under (mirrors
                // `rendezvous_register`). Unregistering a key we aren't
                // registered under is a harmless no-op on the server, so
                // using our current overlay set is safe even if our
                // membership shifted since registration; any stale
                // server-side record TTLs out on its own.
                for key in self.overlay_rendezvous_keys() {
                    self.swarm
                        .behaviour_mut()
                        .rendezvous
                        .unregister(key, *rendezvous_peer);
                }

                self.discovery.state.update_rendezvous_registration_status(
                    rendezvous_peer,
                    RendezvousRegistrationStatus::Expired,
                );
            }
            RendezvousRegistrationStatus::Requested => {
                // Can't cancel in-flight registration, but mark as expired so we don't
                // consider ourselves registered when the response arrives. The handler
                // for the registration response should check current status and re-register
                // with updated addresses if needed.
                self.discovery.state.update_rendezvous_registration_status(
                    rendezvous_peer,
                    RendezvousRegistrationStatus::Expired,
                );
            }
            RendezvousRegistrationStatus::Discovered | RendezvousRegistrationStatus::Expired => {
                // Nothing to unregister
            }
        }

        Ok(())
    }

    // Finds a new rendezvous peer for registration.
    // Prioritizes Discovered peers, falls back to dialing Expired peers if necessary.
    // Returns Some(PeerId) if a suitable peer is found, None otherwise.
    pub(crate) fn find_new_rendezvous_peer(&self) -> Option<PeerId> {
        let mut candidate = None;

        for peer_id in self.discovery.state.get_rendezvous_peer_ids() {
            if let Some(peer_info) = self.discovery.state.get_peer_info(&peer_id) {
                if let Some(rendezvous_info) = peer_info.rendezvous() {
                    match rendezvous_info.registration_status() {
                        RendezvousRegistrationStatus::Discovered => {
                            // If we find a Discovered peer, return it right away
                            return Some(peer_id);
                        }
                        RendezvousRegistrationStatus::Expired if candidate.is_none() => {
                            candidate = Some(peer_id);
                        }
                        RendezvousRegistrationStatus::Requested
                        | RendezvousRegistrationStatus::Registered
                        | RendezvousRegistrationStatus::Expired => {}
                    }
                }
            }
        }

        candidate
    }

    // Requests relay reservation on relay peer if one is required.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn create_relay_reservation(&mut self, relay_peer: &PeerId) -> EyreResult<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(relay_peer)
            .wrap_err("Failed to get peer info")?;

        if !self
            .discovery
            .state
            .is_relay_reservation_required(self.discovery.relay_config.registrations_limit)
        {
            return Ok(());
        }

        debug!(
            %relay_peer,
            ?peer_info,
            "Attempting to register with rendezvous node"
        );

        let preferred_addr = peer_info
            .get_preferred_addr()
            .wrap_err("Failed to get preferred addr for relay peer")?;

        // libp2p's relay-client transport requires the relay address to
        // be the FULL form:
        //   /<transport>/p2p/<relay-peer>/p2p-circuit/p2p/<self-peer>
        //
        // The `/p2p/<relay-peer>` segment between the transport and
        // `/p2p-circuit` is how the relay-client knows which peer is
        // serving the circuit. Without it, `listen_on` rejects the
        // multiaddr with a near-empty transport-level error and the
        // reservation flow silently fails.
        //
        // Why this would land on an incomplete addr: `peer_info.addrs`
        // is populated from two sources — `update_peer_protocols` (the
        // Identify protocols block, which carries no addresses) and the
        // listen_addrs loop in identify.rs which adds the peer's raw
        // listen_addrs. The peer's listen_addrs are bare network
        // endpoints (`/ip4/.../udp/.../quic-v1`) with no `/p2p/`
        // suffix. `get_preferred_addr` prefers UDP, which means it
        // returns the bare endpoint and the constructed relayed addr
        // ends up missing the relay-peer segment.
        //
        // Fix: append `/p2p/<relay-peer>` to the base only if it isn't
        // already present (some entries in `peer_info.addrs` are
        // populated from connection-established events and DO carry
        // the `/p2p/`).
        let mut base_addr = preferred_addr.clone();
        if !base_addr.iter().any(|p| matches!(p, Protocol::P2p(_))) {
            base_addr.push(Protocol::P2p(*relay_peer));
        }

        let relayed_addr = match base_addr
            .with(Protocol::P2pCircuit)
            .with_p2p(*self.swarm.local_peer_id())
        {
            Ok(addr) => addr,
            Err(err) => {
                bail!("Failed to construct relayed addr for relay peer: {:?}", err)
            }
        };

        // Wrap `listen_on` with context — its underlying error type
        // (libp2p's swarm::ListenError → TransportError) renders to
        // an unhelpfully terse string when propagated raw via `?`, and
        // the identify handler's `error!(%err, ...)` printout above
        // this call site reduces to `err=` with no Display content.
        // That made the previous incarnation of this bug
        // (relayed_addr missing /p2p/<relay-peer>) invisible in logs.
        let listener_id = self
            .swarm
            .listen_on(relayed_addr.clone())
            .wrap_err_with(|| format!("Failed to listen on relayed addr {}", relayed_addr))?;

        // Record the listener id so the ListenerClosed handler can route
        // recovery back to this relay peer even if the close event comes
        // with an empty `addresses` list (the quota-denied-before-address-
        // allocation case).
        self.discovery
            .state
            .record_relay_listener(listener_id, *relay_peer);

        self.discovery
            .state
            .update_relay_reservation_status(relay_peer, RelayReservationStatus::Requested);

        Ok(())
    }

    /// Execute actions determined by DiscoveryState
    pub(crate) fn execute_reachability_actions(&mut self, actions: ReachabilityActions) {
        if !actions.has_actions() {
            return;
        }

        // AutoNAT server control
        if actions.enable_autonat_server {
            if let Err(err) = self.swarm.behaviour_mut().autonat.enable_server() {
                error!(%err, "Failed to enable AutoNAT server");
            }
        }

        if actions.disable_autonat_server {
            if let Err(err) = self.swarm.behaviour_mut().autonat.disable_server() {
                error!(%err, "Failed to disable AutoNAT server");
            }
        }

        // Unregister from rendezvous (do this first when going private)
        for peer_id in actions.rendezvous_unregister {
            if let Err(err) = self.rendezvous_unregister(&peer_id) {
                error!(%err, %peer_id, "Failed to unregister from rendezvous");
            }
        }

        // Create relay reservations
        for peer_id in actions.relay_reservations {
            if let Err(err) = self.create_relay_reservation(&peer_id) {
                error!(%err, %peer_id, "Failed to create relay reservation");
            }
        }

        // Discover peers — throttled path
        for peer_id in &actions.rendezvous_discover {
            if let Err(err) = self.rendezvous_discover(peer_id, false) {
                error!(%err, %peer_id, "Failed to discover via rendezvous");
            }
        }

        // Discover peers — force path (bypasses throttle, for
        // event-driven recovery from a lost peer connection where
        // we can't afford to wait the discovery_rpm floor).
        for peer_id in &actions.rendezvous_discover_force {
            if let Err(err) = self.rendezvous_discover(peer_id, true) {
                error!(%err, %peer_id, "Failed to force-discover via rendezvous");
            }
        }

        // Register with rendezvous (do this last, after relay setup)
        for peer_id in actions.rendezvous_register {
            if let Err(err) = self.rendezvous_register(&peer_id) {
                error!(%err, %peer_id, "Failed to register with rendezvous");
            }
        }
    }
}
