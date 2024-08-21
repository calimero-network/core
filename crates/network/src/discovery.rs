use eyre::ContextCompat;
use libp2p::PeerId;
use tracing::{debug, error};

pub(crate) mod state;

use super::{config, EventLoop};

#[derive(Debug)]
pub(crate) struct Discovery {
    pub(crate) state: state::DiscoveryState,
    pub(crate) rendezvous_config: config::RendezvousConfig,
}

impl Discovery {
    pub(crate) fn new(rendezvous_config: &config::RendezvousConfig) -> Self {
        Discovery {
            state: Default::default(),
            rendezvous_config: rendezvous_config.clone(),
        }
    }
}

impl EventLoop {
    // Sends rendezvous discovery requests to all rendezvous peers which are not throttled.
    // If rendezvous peer is not connected, it will be dialed which will trigger the discovery during identify exchange.
    pub(crate) async fn broadcast_rendezvous_discoveries(&mut self) {
        for peer_id in self
            .discovery
            .state
            .get_rendezvous_peer_ids()
            .collect::<Vec<_>>()
        {
            let peer_info = match self.discovery.state.get_peer_info(&peer_id) {
                Some(info) => info,
                None => {
                    error!(%peer_id, "Failed to lookup peer info");
                    continue;
                }
            };

            if peer_info
                .is_rendezvous_discover_throttled(self.discovery.rendezvous_config.discovery_rpm)
            {
                continue;
            }

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial rendezvous peer");
                    }
                }
            } else if let Err(err) = self.rendezvous_discover(&peer_id) {
                error!(%err, "Failed to perform rendezvous discover");
            }
        }
    }

    // Sends rendezvous discovery request to the rendezvous peer if not throttled.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_discover(&mut self, rendezvous_peer: &PeerId) -> eyre::Result<()> {
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
    pub(crate) fn broadcast_rendezvous_registrations(&mut self) -> eyre::Result<()> {
        for peer_id in self
            .discovery
            .state
            .get_rendezvous_peer_ids()
            .collect::<Vec<_>>()
        {
            let peer_info = match self.discovery.state.get_peer_info(&peer_id) {
                Some(info) => info,
                None => {
                    error!(%peer_id, "Failed to lookup peer info");
                    continue;
                }
            };

            if !peer_info.is_rendezvous_registration_required() {
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

        Ok(())
    }

    // Sends rendezvous registration request to rendezvous peer if one is required.
    // If there are no external addresses for the node, the registration is considered successful.
    // This function expectes that the rendezvous peer is already connected.
    pub(crate) fn rendezvous_register(&mut self, rendezvous_peer: &PeerId) -> eyre::Result<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info")?;

        if !peer_info.is_rendezvous_registration_required() {
            return Ok(());
        }

        if let Err(err) = self.swarm.behaviour_mut().rendezvous.register(
            self.discovery.rendezvous_config.namespace.clone(),
            *rendezvous_peer,
            None,
        ) {
            match err {
                libp2p::rendezvous::client::RegisterError::NoExternalAddresses => {
                    return Ok(());
                }
                err => eyre::bail!(err),
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
            state::RendezvousRegistrationStatus::Requested,
        );

        Ok(())
    }

    // Requests relay reservation on relay peer if one is required.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn create_relay_reservation(&mut self, relay_peer: &PeerId) -> eyre::Result<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(relay_peer)
            .wrap_err("Failed to get peer info")?;

        if !peer_info.is_relay_reservation_required() {
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
            .with(multiaddr::Protocol::P2pCircuit)
            .with_p2p(*self.swarm.local_peer_id())
        {
            Ok(addr) => addr,
            Err(err) => {
                eyre::bail!("Failed to construct relayed addr for relay peer: {:?}", err)
            }
        };

        let _ = self.swarm.listen_on(relayed_addr)?;

        self.discovery
            .state
            .update_relay_reservation_status(relay_peer, state::RelayReservationStatus::Requested);

        Ok(())
    }
}
