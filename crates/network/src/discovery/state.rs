#[cfg(test)]
#[path = "../tests/discovery/state.rs"]
mod tests;

use std::collections::{btree_map, BTreeMap, BTreeSet, HashSet};
use std::time;

use libp2p::{rendezvous, Multiaddr, PeerId, StreamProtocol};

// The rendezvous protocol name is not public in libp2p, so we have to define it here.
// source: https://github.com/libp2p/rust-libp2p/blob/a8888a7978f08ec9b8762207bf166193bf312b94/protocols/rendezvous/src/lib.rs#L50C12-L50C92
const RENDEZVOUS_PROTOCOL_NAME: StreamProtocol = StreamProtocol::new("/rendezvous/1.0.0");

/// DiscoveryState is a struct that holds the state of the disovered peers.
/// It holds the relay and rendezvous indexes to quickly check if a peer is a relay or rendezvous.
/// It offers mutable methods for managing the state of the peers.
#[derive(Debug, Default)]
pub(crate) struct DiscoveryState {
    peers: BTreeMap<PeerId, PeerInfo>,
    relay_index: BTreeSet<PeerId>,
    rendezvous_index: BTreeSet<PeerId>,
}

impl DiscoveryState {
    pub(crate) fn add_peer_addr(&mut self, peer_id: PeerId, addr: &Multiaddr) {
        let _ = self
            .peers
            .entry(peer_id)
            .or_default()
            .addrs
            .insert(addr.clone());
    }

    pub(crate) fn remove_peer(&mut self, peer_id: &PeerId) {
        drop(self.peers.remove(peer_id));
        let _ = self.relay_index.remove(peer_id);
        let _ = self.rendezvous_index.remove(peer_id);
    }

    pub(crate) fn update_peer_protocols(&mut self, peer_id: &PeerId, protocols: &[StreamProtocol]) {
        protocols.iter().for_each(|protocol| {
            if protocol == &libp2p::relay::HOP_PROTOCOL_NAME {
                let _ = self.relay_index.insert(*peer_id);

                match self.peers.entry(*peer_id) {
                    btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().relay.is_none() {
                            entry.get_mut().relay = Some(PeerRelayInfo::default());
                        }
                    }
                    btree_map::Entry::Vacant(entry) => {
                        let _ = entry.insert(PeerInfo {
                            addrs: HashSet::default(),
                            discoveries: HashSet::default(),
                            relay: Some(PeerRelayInfo::default()),
                            rendezvous: None,
                        });
                    }
                };
            }
            if protocol == &RENDEZVOUS_PROTOCOL_NAME {
                let _ = self.rendezvous_index.insert(*peer_id);

                match self.peers.entry(*peer_id) {
                    btree_map::Entry::Occupied(mut entry) => {
                        if entry.get().rendezvous.is_none() {
                            entry.get_mut().rendezvous = Some(PeerRendezvousInfo::default());
                        }
                    }
                    btree_map::Entry::Vacant(entry) => {
                        let _ = entry.insert(PeerInfo {
                            addrs: HashSet::default(),
                            discoveries: HashSet::default(),
                            relay: None,
                            rendezvous: Some(PeerRendezvousInfo::default()),
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
            Some(info) => info.discoveries.contains(&mechanism),
            None => false,
        }
    }

    pub(crate) fn add_peer_discovery_mechanism(
        &mut self,
        peer_id: &PeerId,
        mechanism: PeerDiscoveryMechanism,
    ) {
        match self.peers.entry(*peer_id) {
            btree_map::Entry::Occupied(mut entry) => {
                entry.get_mut().add_discovery_mechanism(mechanism);
            }
            btree_map::Entry::Vacant(entry) => {
                let mut discoveries = HashSet::new();
                let _ = discoveries.insert(mechanism);

                let _ = entry.insert(PeerInfo {
                    addrs: HashSet::default(),
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
        cookie: &rendezvous::Cookie,
    ) {
        let _ = self
            .peers
            .entry(*rendezvous_peer)
            .and_modify(|info| info.update_rendezvous_cookie(cookie.clone()));
    }

    pub(crate) fn update_relay_reservation_status(
        &mut self,
        relay_peer: &PeerId,
        status: RelayReservationStatus,
    ) {
        let _ = self
            .peers
            .entry(*relay_peer)
            .and_modify(|info| info.update_relay_reservation_status(status));
    }

    pub(crate) fn update_rendezvous_registration_status(
        &mut self,
        rendezvous_peer: &PeerId,
        status: RendezvousRegistrationStatus,
    ) {
        let _ = self
            .peers
            .entry(*rendezvous_peer)
            .and_modify(|info| info.update_rendezvous_registartion_status(status));
    }

    pub(crate) fn get_peer_info(&self, peer_id: &PeerId) -> Option<&PeerInfo> {
        self.peers.get(peer_id)
    }

    pub(crate) fn get_rendezvous_peer_ids(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.rendezvous_index.iter().copied()
    }

    pub(crate) fn is_peer_relay(&self, peer_id: &PeerId) -> bool {
        self.relay_index.contains(peer_id)
    }

    pub(crate) fn is_peer_rendezvous(&self, peer_id: &PeerId) -> bool {
        self.rendezvous_index.contains(peer_id)
    }
}

/// PeerInfo is a struct that holds information about a peer.
/// It offers immutable methods for accessing the information.
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
        self.relay.as_ref().map_or(true, |info| {
            matches!(
                info.reservation_status(),
                RelayReservationStatus::Discovered | RelayReservationStatus::Expired
            )
        })
    }

    pub(crate) fn is_rendezvous_discover_throttled(&self, rpm: f32) -> bool {
        self.rendezvous.as_ref().map_or(false, |info| {
            info.last_discovery_at().map_or(false, |instant| {
                instant.elapsed() < time::Duration::from_secs_f32(60.0 / rpm)
            })
        })
    }

    pub(crate) fn is_rendezvous_registration_required(&self) -> bool {
        self.rendezvous.as_ref().map_or(true, |info| {
            matches!(
                info.registration_status(),
                RendezvousRegistrationStatus::Discovered | RendezvousRegistrationStatus::Expired
            )
        })
    }

    pub(crate) fn rendezvous(&self) -> Option<&PeerRendezvousInfo> {
        self.rendezvous.as_ref()
    }

    fn add_discovery_mechanism(&mut self, mechanism: PeerDiscoveryMechanism) {
        let _ = self.discoveries.insert(mechanism);
    }

    fn update_rendezvous_cookie(&mut self, cookie: rendezvous::Cookie) {
        if let Some(ref mut info) = self.rendezvous {
            info.update_cookie(cookie);
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
