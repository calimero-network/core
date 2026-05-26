use core::net::Ipv4Addr;
use core::time::Duration;
use std::collections::HashSet;

use calimero_network_primitives::config::{AutonatConfig, RelayConfig, RendezvousConfig};
use eyre::{bail, ContextCompat, Result as EyreResult, WrapErr as _};
use libp2p::rendezvous::client::RegisterError;
use libp2p::PeerId;
use multiaddr::{Multiaddr, Protocol};
use tracing::{debug, error};

use super::NetworkManager;
use crate::discovery::state::{
    DiscoveryState, ReachabilityActions, RelayReservationStatus, RendezvousRegistrationStatus,
};

pub mod state;

#[derive(Debug)]
pub struct Discovery {
    pub(crate) state: DiscoveryState,
    pub(crate) rendezvous_config: RendezvousConfig,
    pub(crate) relay_config: RelayConfig,
    pub(crate) advertise: Option<AdvertiseState>,
    pub(crate) _autonat_config: AutonatConfig,
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
    // Sends rendezvous discovery request to the rendezvous peer if not throttled.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_discover(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info {}")?;

        if peer_info
            .is_rendezvous_discover_throttled(self.discovery.rendezvous_config.discovery_rpm)
        {
            return Ok(());
        }

        self.swarm.behaviour_mut().rendezvous.discover(
            Some(self.discovery.rendezvous_config.namespace.clone()),
            peer_info
                .rendezvous()
                .and_then(|info| info.cookie())
                .cloned(),
            None,
            *rendezvous_peer,
        );

        debug!(
            %rendezvous_peer,
            ?peer_info,
            rendezvous_namespace=%(self.discovery.rendezvous_config.namespace),
            "Sent discover request to rendezvous node"
        );

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

    // Sends rendezvous registration request to rendezvous peer if one is required.
    // If there are no external addresses for the node, the registration is considered successful.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_register(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info")?;

        if !self.discovery.state.is_rendezvous_registration_required(
            self.discovery.rendezvous_config.registrations_limit,
        ) {
            return Ok(());
        }

        if let Err(err) = self.swarm.behaviour_mut().rendezvous.register(
            self.discovery.rendezvous_config.namespace.clone(),
            *rendezvous_peer,
            None,
        ) {
            match err {
                RegisterError::NoExternalAddresses => {
                    debug!("No external addresses to register at rendezvous");
                    return Ok(());
                }
                err @ RegisterError::FailedToMakeRecord(_) => {
                    bail!(err)
                }
            }
        }

        debug!(
            %rendezvous_peer,
            ?peer_info,
            rendezvous_namespace=%(self.discovery.rendezvous_config.namespace),
            "Sent register request to rendezvous node"
        );

        self.discovery.state.update_rendezvous_registration_status(
            rendezvous_peer,
            RendezvousRegistrationStatus::Requested,
        );

        Ok(())
    }

    // We unregister from a rendezvous peer if we were previously registered.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_unregister(&mut self, rendezvous_peer: &PeerId) -> EyreResult<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info")?
            .rendezvous()
            .wrap_err("Peer isn't rendezvous")?;

        match peer_info.registration_status() {
            RendezvousRegistrationStatus::Registered => {
                // Actively unregister from the rendezvous server
                self.swarm.behaviour_mut().rendezvous.unregister(
                    self.discovery.rendezvous_config.namespace.clone(),
                    *rendezvous_peer,
                );

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

        // Discover peers
        for peer_id in &actions.rendezvous_discover {
            if let Err(err) = self.rendezvous_discover(peer_id) {
                error!(%err, %peer_id, "Failed to discover via rendezvous");
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
