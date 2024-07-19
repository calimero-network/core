use std::collections::{btree_map, BTreeMap, BTreeSet, HashSet};
use std::time;

use libp2p::{rendezvous, Multiaddr, PeerId, StreamProtocol};

// The rendezvous protocol name is not public in libp2p, so we have to define it here.
// source: https://github.com/libp2p/rust-libp2p/blob/a8888a7978f08ec9b8762207bf166193bf312b94/protocols/rendezvous/src/lib.rs#L50C12-L50C92
const RENDEZVOUS_PROTOCOL_NAME: libp2p::StreamProtocol =
    libp2p::StreamProtocol::new("/rendezvous/1.0.0");

#[derive(Debug, Default)]
pub(crate) struct DiscoveryState {
    peers: BTreeMap<PeerId, PeerInfo>,
    relay_index: BTreeSet<PeerId>,
    rendezvous_index: BTreeSet<PeerId>,
}

impl DiscoveryState {
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

    pub(crate) fn update_peer_protocols(&mut self, peer_id: &PeerId, protocols: &[StreamProtocol]) {
        protocols.iter().for_each(|protocol| {
            if protocol == &libp2p::relay::HOP_PROTOCOL_NAME {
                self.relay_index.insert(*peer_id);

                match self.peers.entry(*peer_id) {
                    btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().relay.is_none() {
                            entry.get_mut().relay = Some(Default::default());
                        }
                    }
                    btree_map::Entry::Vacant(entry) => {
                        entry.insert(PeerInfo {
                            addrs: Default::default(),
                            discoveries: Default::default(),
                            relay: Some(Default::default()),
                            rendezvous: None,
                        });
                    }
                };
            }
            if protocol == &RENDEZVOUS_PROTOCOL_NAME {
                self.rendezvous_index.insert(*peer_id);

                match self.peers.entry(*peer_id) {
                    btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().rendezvous.is_none() {
                            entry.get_mut().rendezvous = Some(Default::default());
                        }
                    }
                    btree_map::Entry::Vacant(entry) => {
                        entry.insert(PeerInfo {
                            addrs: Default::default(),
                            discoveries: Default::default(),
                            relay: None,
                            rendezvous: Some(Default::default()),
                        });
                    }
                };
            }
        });
    }

    pub(crate) fn is_peer_discovered_via(
        &self,
        peer_id: &PeerId,
        mechanism: PeerDiscoveryMechanism,
    ) -> bool {
        match self.peers.get(peer_id) {
            Some(info) => {
                if info.discoveries.contains(&mechanism) {
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    pub(crate) fn add_peer_discovery_mechanism(
        &mut self,
        peer_id: &PeerId,
        mechanism: PeerDiscoveryMechanism,
    ) {
        match self.peers.entry(*peer_id) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().add_discovery_mechanism(mechanism);
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                let mut discoveries = HashSet::new();
                discoveries.insert(mechanism);

                entry.insert(PeerInfo {
                    addrs: Default::default(),
                    discoveries,
                    relay: None,
                    rendezvous: None,
                });
            }
        }
    }

    pub(crate) fn update_rendezvous_cookie(
        &mut self,
        rendezvous_peer: &PeerId,
        cookie: rendezvous::Cookie,
    ) {
        self.peers
            .entry(*rendezvous_peer)
            .and_modify(|info| info.update_rendezvous_cookie(cookie.clone()));
    }

    pub(crate) fn update_relay_reservation_status(
        &mut self,
        relay_peer: &PeerId,
        status: RelayReservationStatus,
    ) {
        self.peers
            .entry(*relay_peer)
            .and_modify(|info| info.update_relay_reservation_status(status));
    }

    pub(crate) fn update_rendezvous_registration_status(
        &mut self,
        rendezvous_peer: &PeerId,
        status: RendezvousRegistrationStatus,
    ) {
        self.peers
            .entry(*rendezvous_peer)
            .and_modify(|info| info.update_rendezvous_registartion_status(status));
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
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PeerInfo {
    addrs: HashSet<Multiaddr>,
    discoveries: HashSet<PeerDiscoveryMechanism>,
    relay: Option<PeerRelayInfo>,
    rendezvous: Option<PeerRendezvousInfo>,
}

impl PeerInfo {
    pub(crate) fn addrs(&self) -> impl Iterator<Item = &Multiaddr> {
        self.addrs.iter()
    }

    pub(crate) fn get_preferred_addr(&self) -> Option<&Multiaddr> {
        let udp_addrs: Vec<&Multiaddr> = self
            .addrs
            .iter()
            .filter(|addr| {
                addr.iter()
                    .any(|p| matches!(p, multiaddr::Protocol::Udp(_)))
            })
            .collect();

        match udp_addrs.len() {
            0 => self.addrs.iter().next(),
            _ => Some(udp_addrs[0]),
        }
    }

    pub(crate) fn is_relay_reservation_required(&self) -> bool {
        self.relay
            .as_ref()
            .map_or(true, |info| match info.reservation_status() {
                RelayReservationStatus::Discovered => true,
                RelayReservationStatus::Expired => true,
                _ => false,
            })
    }

    pub(crate) fn is_rendezvous_discover_throttled(&self, rpm: f32) -> bool {
        self.rendezvous.as_ref().map_or(false, |info| {
            info.last_discovery_at().map_or(false, |instant| {
                instant.elapsed() > time::Duration::from_secs_f32(60.0 / rpm)
            })
        })
    }

    pub(crate) fn is_rendezvous_registration_required(&self) -> bool {
        self.rendezvous
            .as_ref()
            .map_or(true, |info| match info.registration_status() {
                RendezvousRegistrationStatus::Discovered => true,
                RendezvousRegistrationStatus::Expired => true,
                _ => false,
            })
    }

    pub(crate) fn rendezvous(&self) -> Option<&PeerRendezvousInfo> {
        self.rendezvous.as_ref()
    }

    fn add_discovery_mechanism(&mut self, mechanism: PeerDiscoveryMechanism) {
        self.discoveries.insert(mechanism);
    }

    fn update_rendezvous_cookie(&mut self, cookie: rendezvous::Cookie) {
        if let Some(ref mut rendezvous_info) = self.rendezvous {
            rendezvous_info.update_cookie(cookie);
        }
    }

    fn update_relay_reservation_status(&mut self, status: RelayReservationStatus) {
        if let Some(ref mut info) = self.relay {
            info.update_reservation_status(status);
        }
    }

    fn update_rendezvous_registartion_status(&mut self, status: RendezvousRegistrationStatus) {
        if let Some(ref mut info) = self.rendezvous {
            info.update_registration_status(status);
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum PeerDiscoveryMechanism {
    Mdns,
    Rendezvous,
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

#[derive(Clone, Debug, Default)]
pub(crate) struct PeerRendezvousInfo {
    cookie: Option<rendezvous::Cookie>,
    last_discovery_at: Option<time::Instant>,
    registration_status: RendezvousRegistrationStatus,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum RendezvousRegistrationStatus {
    #[default]
    Discovered,
    Requested,
    Registered,
    Expired,
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

    pub(crate) fn registration_status(&self) -> RendezvousRegistrationStatus {
        self.registration_status
    }

    fn update_registration_status(&mut self, status: RendezvousRegistrationStatus) {
        self.registration_status = status;
    }
}
