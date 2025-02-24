use eyre::{bail, ContextCompat, Result as EyreResult};
use libp2p::rendezvous::client::RegisterError;
use libp2p::PeerId;
use multiaddr::Protocol;
use tracing::{debug, error};

use super::NetworkManager;
use crate::config::{RelayConfig, RendezvousConfig};
use crate::discovery::state::{
    DiscoveryState, RelayReservationStatus, RendezvousRegistrationStatus,
};

pub mod state;

#[derive(Debug)]
pub struct Discovery {
    pub(crate) state: DiscoveryState,
    pub(crate) rendezvous_config: RendezvousConfig,
    pub(crate) relay_config: RelayConfig,
}

impl Discovery {
    pub(crate) fn new(rendezvous_config: &RendezvousConfig, relay_config: &RelayConfig) -> Self {
        Self {
            state: DiscoveryState::default(),
            rendezvous_config: rendezvous_config.clone(),
            relay_config: relay_config.clone(),
        }
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

        let relayed_addr = match preferred_addr
            .clone()
            .with(Protocol::P2pCircuit)
            .with_p2p(*self.swarm.local_peer_id())
        {
            Ok(addr) => addr,
            Err(err) => {
                bail!("Failed to construct relayed addr for relay peer: {:?}", err)
            }
        };

        let _ = self.swarm.listen_on(relayed_addr)?;

        self.discovery
            .state
            .update_relay_reservation_status(relay_peer, RelayReservationStatus::Requested);

        Ok(())
    }
}
