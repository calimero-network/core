use std::time;

use eyre::ContextCompat;
use libp2p::PeerId;
use tracing::{debug, error};

pub(crate) mod state;
use super::{config, EventLoop};

#[derive(Debug)]
pub(crate) struct Discovery {
    pub(crate) rendezvous_config: config::RendezvousConfig,
    pub(crate) state: state::DiscoveryState,
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
    // Handles rendezvous discoveries for all rendezvous peers.
    // If rendezvous peer is not connected, it will be dialed which will trigger the discovery during identify exchange.
    pub(crate) async fn handle_rendezvous_discoveries(&mut self) {
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

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial rendezvous peer");
                    }
                }
            } else {
                if let Err(err) = self.perform_rendezvous_discovery(&peer_id) {
                    error!(%err, "Failed to perform rendezvous discover");
                }
            }
        }
    }

    // Performs rendezvous discovery against the remote rendezvous peer if it's time to do so.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn perform_rendezvous_discovery(
        &mut self,
        rendezvous_peer: &PeerId,
    ) -> eyre::Result<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info {}")?;

        let is_throttled = peer_info.rendezvous().map_or(false, |info| {
            info.last_discovery_at().map_or(false, |instant| {
                instant.elapsed()
                    > time::Duration::from_secs_f32(
                        60.0 / self.discovery.rendezvous_config.discovery_rpm,
                    )
            })
        });

        debug!(
            %rendezvous_peer,
            ?is_throttled,
            "Checking if rendezvous discovery is throttled"
        );

        if !is_throttled {
            self.swarm.behaviour_mut().rendezvous.discover(
                Some(self.discovery.rendezvous_config.namespace.clone()),
                peer_info
                    .rendezvous()
                    .and_then(|info| info.cookie())
                    .cloned(),
                None,
                *rendezvous_peer,
            );
        }

        Ok(())
    }

    // Broadcasts rendezvous registrations to all rendezvous peers if there are pending address changes.
    // If rendezvous peer is not connected, it will be dialed which will trigger the registration during identify exchange.
    pub(crate) fn broadcast_rendezvous_registrations(&mut self) -> eyre::Result<()> {
        if !self.discovery.state.pending_addr_changes() {
            return Ok(());
        }

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

            if !self.swarm.is_connected(&peer_id) {
                for addr in peer_info.addrs().cloned() {
                    if let Err(err) = self.swarm.dial(addr) {
                        error!(%err, "Failed to dial relay peer");
                    }
                }
            } else {
                if let Err(err) = self.update_rendezvous_registration(&peer_id) {
                    error!(%err, "Failed to update rendezvous registration");
                }
            }
        }

        self.discovery.state.clear_pending_addr_changes();

        Ok(())
    }

    // Updates rendezvous registration on the remote rendezvous peer.
    // If there are no external addresses for the node, the registration is considered successful.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn update_rendezvous_registration(&mut self, peer_id: &PeerId) -> eyre::Result<()> {
        if let Err(err) = self.swarm.behaviour_mut().rendezvous.register(
            self.discovery.rendezvous_config.namespace.clone(),
            *peer_id,
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
            %peer_id, rendezvous_namespace=%(self.discovery.rendezvous_config.namespace),
            "Sent register request to rendezvous node"
        );

        Ok(())
    }

    // Creates relay reservation if node didn't already request addres relayed address on the relay peer.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn create_relay_reservation(&mut self, peer_id: &PeerId) -> eyre::Result<()> {
        let peer_info = self
            .discovery
            .state
            .get_peer_info(peer_id)
            .wrap_err("Failed to get peer info")?;

        let is_relay_reservation_required = match peer_info.relay() {
            Some(info) => match info.reservation_status() {
                state::RelayReservationStatus::Discovered => true,
                state::RelayReservationStatus::Expired => true,
                _ => false,
            },
            None => true,
        };
        debug!(
            ?peer_info,
            %is_relay_reservation_required,
            "Checking if relay reservation is required"
        );

        if !is_relay_reservation_required {
            return Ok(());
        }

        let preferred_addr = peer_info
            .get_preferred_addr()
            .wrap_err("Failed to get preferred addr for relay peer")?;

        let relayed_addr = match preferred_addr
            .clone()
            .with(multiaddr::Protocol::P2pCircuit)
            .with_p2p(self.swarm.local_peer_id().clone())
        {
            Ok(addr) => addr,
            Err(err) => {
                eyre::bail!("Failed to construct relayed addr for relay peer: {:?}", err)
            }
        };
        self.swarm.listen_on(relayed_addr)?;
        self.discovery
            .state
            .update_relay_reservation_status(&peer_id, state::RelayReservationStatus::Requested);

        Ok(())
    }
}
