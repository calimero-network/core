use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::{self, Duration};

use eyre::ContextCompat;
use libp2p::{rendezvous, Multiaddr, PeerId, StreamProtocol};
use tracing::{debug, error};

use super::{config, EventLoop};

// The rendezvous protocol name is not public in libp2p, so we have to define it here.
// source: https://github.com/libp2p/rust-libp2p/blob/a8888a7978f08ec9b8762207bf166193bf312b94/protocols/rendezvous/src/lib.rs#L50C12-L50C92
const RENDEZVOUS_PROTOCOL_NAME: libp2p::StreamProtocol =
    libp2p::StreamProtocol::new("/rendezvous/1.0.0");

#[derive(Debug)]
pub(crate) struct DiscoveryState {
    peers: BTreeMap<PeerId, PeerInfo>,
    relay_index: BTreeSet<PeerId>,
    rendezvous_index: BTreeSet<PeerId>,
    rendezvous_config: RendezvousConfig,
    pending_addr_changes: bool,
}

#[derive(Debug)]
struct RendezvousConfig {
    namespace: rendezvous::Namespace,
    rpm: f32,
}

impl From<&config::RendezvousConfig> for RendezvousConfig {
    fn from(config: &config::RendezvousConfig) -> Self {
        Self {
            namespace: config.namespace.clone(),
            rpm: config.discovery_rpm,
        }
    }
}

impl DiscoveryState {
    pub(crate) fn new(rendezvous_config: &config::RendezvousConfig) -> Self {
        DiscoveryState {
            peers: Default::default(),
            relay_index: Default::default(),
            rendezvous_index: Default::default(),
            rendezvous_config: rendezvous_config.into(),
            pending_addr_changes: false,
        }
    }

    pub(crate) fn add_peer_addr(&mut self, peer_id: PeerId, addr: &Multiaddr) {
        self.peers
            .entry(peer_id)
            .or_default()
            .addrs
            .insert(addr.clone());
    }

    pub(crate) fn remove_peer(&mut self, peer_id: &PeerId) {
        self.peers.remove(peer_id);
        self.relay_index.remove(peer_id);
        self.rendezvous_index.remove(peer_id);
    }

    pub(crate) fn update_peer_protocols(
        &mut self,
        peer_id: &PeerId,
        protocols: Vec<StreamProtocol>,
    ) {
        protocols.iter().for_each(|protocol| {
            if protocol == &libp2p::relay::HOP_PROTOCOL_NAME {
                self.relay_index.insert(*peer_id);
                self.peers.entry(*peer_id).or_default().relay = Some(PeerRelayInfo {
                    reservation_status: Default::default(),
                });
            }
            if protocol == &RENDEZVOUS_PROTOCOL_NAME {
                self.rendezvous_index.insert(*peer_id);
                self.peers.entry(*peer_id).or_default().rendezvous = Some(PeerRendezvousInfo {
                    cookie: None,
                    last_discovery_at: None,
                });
            }
        });
    }

    pub(crate) fn update_rendezvous_cookie(
        &mut self,
        rendezvous_peer: &PeerId,
        cookie: rendezvous::Cookie,
    ) {
        self.peers
            .entry(*rendezvous_peer)
            .and_modify(|info| info.update_rendezvous_cookie(cookie.clone()))
            .or_default()
            .rendezvous = Some(PeerRendezvousInfo {
            cookie: Some(cookie.clone()),
            last_discovery_at: None,
        });
    }

    pub(crate) fn update_relay_reservation_status(
        &mut self,
        relay_peer: &PeerId,
        status: RelayReservationStatus,
    ) {
        self.peers
            .entry(*relay_peer)
            .and_modify(|info| info.update_relay_reservation_status(status))
            .or_default()
            .relay = Some(PeerRelayInfo {
            reservation_status: status,
        });
    }

    pub(crate) fn get_peer_info(&self, peer_id: &PeerId) -> Option<&PeerInfo> {
        self.peers.get(peer_id)
    }

    pub(crate) fn get_rendezvous_peer_ids(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.rendezvous_index.iter().cloned()
    }

    pub(crate) fn is_peer_relay(&self, peer_id: &PeerId) -> bool {
        self.relay_index.contains(peer_id)
    }

    pub(crate) fn is_peer_rendezvous(&self, peer_id: &PeerId) -> bool {
        self.rendezvous_index.contains(peer_id)
    }

    pub(crate) fn pending_addr_changes(&self) -> bool {
        self.pending_addr_changes
    }

    pub(crate) fn set_pending_addr_changes(&mut self) {
        self.pending_addr_changes = true;
    }

    pub(crate) fn clear_pending_addr_changes(&mut self) {
        self.pending_addr_changes = true;
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PeerInfo {
    addrs: HashSet<Multiaddr>,
    relay: Option<PeerRelayInfo>,
    rendezvous: Option<PeerRendezvousInfo>,
}

impl PeerInfo {
    pub(crate) fn addrs(&self) -> impl Iterator<Item = &Multiaddr> {
        self.addrs.iter()
    }

    pub(crate) fn get_preferred_addr(&self) -> Option<Multiaddr> {
        let udp_addrs: Vec<&Multiaddr> = self
            .addrs
            .iter()
            .filter(|addr| {
                addr.iter()
                    .any(|p| matches!(p, multiaddr::Protocol::Udp(_)))
            })
            .collect();

        match udp_addrs.len() {
            0 => self.addrs.iter().next().cloned(),
            _ => Some(udp_addrs[0].clone()),
        }
    }

    pub(crate) fn relay(&self) -> Option<&PeerRelayInfo> {
        self.relay.as_ref()
    }

    pub(crate) fn rendezvous(&self) -> Option<&PeerRendezvousInfo> {
        self.rendezvous.as_ref()
    }

    fn update_rendezvous_cookie(&mut self, cookie: rendezvous::Cookie) {
        if let Some(ref mut rendezvous_info) = self.rendezvous {
            rendezvous_info.update_cookie(cookie);
        }
    }

    fn update_relay_reservation_status(&mut self, status: RelayReservationStatus) {
        if let Some(ref mut relay_info) = self.relay {
            relay_info.update_reservation_status(status);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PeerRelayInfo {
    reservation_status: RelayReservationStatus,
}

impl PeerRelayInfo {
    pub(crate) fn reservation_status(&self) -> RelayReservationStatus {
        self.reservation_status
    }

    fn update_reservation_status(&mut self, status: RelayReservationStatus) {
        self.reservation_status = status;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum RelayReservationStatus {
    #[default]
    Discovered,
    Requested,
    Accepted,
    Expired,
}

#[derive(Clone, Debug)]
pub(crate) struct PeerRendezvousInfo {
    cookie: Option<rendezvous::Cookie>,
    last_discovery_at: Option<time::Instant>,
}

impl PeerRendezvousInfo {
    pub(crate) fn cookie(&self) -> Option<&rendezvous::Cookie> {
        self.cookie.as_ref()
    }

    pub(crate) fn last_discovery_at(&self) -> Option<time::Instant> {
        self.last_discovery_at
    }

    fn update_cookie(&mut self, cookie: rendezvous::Cookie) {
        self.cookie = Some(cookie);
        self.last_discovery_at = Some(time::Instant::now());
    }
}

impl EventLoop {
    // Handles rendezvous discoveries for all rendezvous peers.
    // If rendezvous peer is not connected, it will be dialed which will trigger the discovery during identify exchange.
    pub(crate) async fn handle_rendezvous_discoveries(&mut self) {
        for peer_id in self
            .discovery_state
            .get_rendezvous_peer_ids()
            .collect::<Vec<_>>()
        {
            let peer_info = match self.discovery_state.get_peer_info(&peer_id) {
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
            .discovery_state
            .get_peer_info(rendezvous_peer)
            .wrap_err("Failed to get peer info {}")?;

        let is_throttled = peer_info.rendezvous().map_or(false, |info| {
            info.last_discovery_at().map_or(false, |instant| {
                instant.elapsed()
                    > Duration::from_secs_f32(60.0 / self.discovery_state.rendezvous_config.rpm)
            })
        });

        debug!(
            %rendezvous_peer,
            ?is_throttled,
            "Checking if rendezvous discovery is throttled"
        );

        if !is_throttled {
            self.swarm.behaviour_mut().rendezvous.discover(
                Some(self.discovery_state.rendezvous_config.namespace.clone()),
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
        if !self.discovery_state.pending_addr_changes() {
            return Ok(());
        }

        for peer_id in self
            .discovery_state
            .get_rendezvous_peer_ids()
            .collect::<Vec<_>>()
        {
            let peer_info = match self.discovery_state.get_peer_info(&peer_id) {
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

        self.discovery_state.clear_pending_addr_changes();

        Ok(())
    }

    // Updates rendezvous registration on the remote rendezvous peer.
    // If there are no external addresses for the node, the registration is considered successful.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn update_rendezvous_registration(&mut self, peer_id: &PeerId) -> eyre::Result<()> {
        if let Err(err) = self.swarm.behaviour_mut().rendezvous.register(
            self.discovery_state.rendezvous_config.namespace.clone(),
            *peer_id,
            None,
        ) {
            match err {
                libp2p::rendezvous::client::RegisterError::NoExternalAddresses => {}
                err => eyre::bail!(err),
            }
        }

        debug!(
            %peer_id, rendezvous_namespace=%(self.discovery_state.rendezvous_config.namespace),
            "Sent register request to rendezvous node"
        );
        Ok(())
    }

    // Creates relay reservation if node didn't already request addres relayed address on the relay peer.
    // This function expectes that the relay peer is already connected.
    pub(crate) fn create_relay_reservation(&mut self, peer_id: &PeerId) -> eyre::Result<()> {
        let peer_info = self
            .discovery_state
            .get_peer_info(peer_id)
            .wrap_err("Failed to get peer info")?;

        let is_relay_reservation_required = match peer_info.relay() {
            Some(info) => match info.reservation_status() {
                RelayReservationStatus::Discovered => true,
                RelayReservationStatus::Expired => true,
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
        self.discovery_state
            .update_relay_reservation_status(&peer_id, RelayReservationStatus::Requested);

        Ok(())
    }
}
