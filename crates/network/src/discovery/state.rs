#[cfg(test)]
#[path = "../tests/discovery/state.rs"]
mod tests;

use core::time::Duration;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::time::Instant;

use libp2p::autonat::{NatStatus, DEFAULT_PROTOCOL_NAME as AUTONAT_PROTOCOL_NAME};
use libp2p::relay::HOP_PROTOCOL_NAME;
use libp2p::rendezvous::Cookie;
use libp2p::{Multiaddr, PeerId, StreamProtocol};
use multiaddr::Protocol;

// The rendezvous protocol name is not public in libp2p, so we have to define it here.
// source: https://github.com/libp2p/rust-libp2p/blob/a8888a7978f08ec9b8762207bf166193bf312b94/protocols/rendezvous/src/lib.rs#L50C12-L50C92
const RENDEZVOUS_PROTOCOL_NAME: StreamProtocol = StreamProtocol::new("/rendezvous/1.0.0");

/// DiscoveryState is a struct that holds the state of the disovered peers.
/// It holds the relay and rendezvous indexes to quickly check if a peer is a relay or rendezvous.
/// It offers mutable methods for managing the state of the peers.
#[derive(Debug, Default)]
pub struct DiscoveryState {
    peers: BTreeMap<PeerId, PeerInfo>,
    relay_index: BTreeSet<PeerId>,
    rendezvous_index: BTreeSet<PeerId>,
    autonat_index: BTreeSet<PeerId>,
    autonat: AutonatStatus,
}
#[derive(Debug)]
pub struct AutonatStatus {
    status: NatStatus,
    last_status_public: bool,
}

impl Default for AutonatStatus {
    fn default() -> Self {
        Self {
            status: NatStatus::Unknown,
            last_status_public: false,
        }
    }
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
        for protocol in protocols {
            if protocol == &HOP_PROTOCOL_NAME {
                let _ = self.relay_index.insert(*peer_id);

                let peer_info = self.peers.entry(*peer_id).or_insert_with(Default::default);
                let _ignored = peer_info.relay.get_or_insert_with(Default::default);
            }
            if protocol == &RENDEZVOUS_PROTOCOL_NAME {
                let _ = self.rendezvous_index.insert(*peer_id);

                let peer_info = self.peers.entry(*peer_id).or_insert_with(Default::default);
                let _ignored = peer_info.rendezvous.get_or_insert_with(Default::default);
            }

            if protocol == &AUTONAT_PROTOCOL_NAME {
                let _ = self.autonat_index.insert(*peer_id);

                let _peer_info = self.peers.entry(*peer_id).or_insert_with(Default::default);
            }
        }
    }

    pub(crate) fn is_peer_discovered_via(
        &self,
        peer_id: &PeerId,
        mechanism: PeerDiscoveryMechanism,
    ) -> bool {
        self.peers
            .get(peer_id)
            .map_or(false, |info| info.discoveries.contains(&mechanism))
    }

    pub(crate) fn add_peer_discovery_mechanism(
        &mut self,
        peer_id: &PeerId,
        mechanism: PeerDiscoveryMechanism,
    ) {
        match self.peers.entry(*peer_id) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().add_discovery_mechanism(mechanism);
            }
            Entry::Vacant(entry) => {
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

    pub(crate) fn update_rendezvous_cookie(&mut self, rendezvous_peer: &PeerId, cookie: &Cookie) {
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

    pub(crate) fn get_relay_peer_ids(&self) -> impl Iterator<Item = PeerId> + '_ {
        self.relay_index.iter().copied()
    }

    pub(crate) fn is_peer_relay(&self, peer_id: &PeerId) -> bool {
        self.relay_index.contains(peer_id)
    }

    pub(crate) fn is_peer_rendezvous(&self, peer_id: &PeerId) -> bool {
        self.rendezvous_index.contains(peer_id)
    }

    // TOOD: Revisit AutoNAT protocol integration
    // pub(crate) fn is_peer_autonat(&self, peer_id: &PeerId) -> bool {
    //     self.autonat_index.contains(peer_id)
    // }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Cannot use saturating_add() due to non-specific integer type"
    )]
    pub(crate) fn is_rendezvous_registration_required(&self, max: usize) -> bool {
        let sum = self
            .get_rendezvous_peer_ids()
            .filter_map(|peer_id| self.get_peer_info(&peer_id))
            .fold(0, |acc, peer_info| {
                peer_info.rendezvous().map_or(acc, |rendezvous_info| {
                    match rendezvous_info.registration_status() {
                        RendezvousRegistrationStatus::Requested
                        | RendezvousRegistrationStatus::Registered => acc + 1,
                        RendezvousRegistrationStatus::Discovered
                        | RendezvousRegistrationStatus::Expired => acc,
                    }
                })
            });
        sum < max
    }

    #[expect(
        clippy::arithmetic_side_effects,
        reason = "Cannot use saturating_add() due to non-specific integer type"
    )]
    pub(crate) fn is_relay_reservation_required(&self, max: usize) -> bool {
        let sum = self
            .get_relay_peer_ids()
            .filter_map(|peer_id| self.get_peer_info(&peer_id))
            .fold(0, |acc, peer_info| {
                peer_info.relay().map_or(acc, |rendezvous_info| {
                    match rendezvous_info.reservation_status() {
                        RelayReservationStatus::Accepted | RelayReservationStatus::Requested => {
                            acc + 1
                        }
                        RelayReservationStatus::Discovered | RelayReservationStatus::Expired => acc,
                    }
                })
            });
        sum < max
    }

    pub(crate) fn update_autonat_status(&mut self, status: NatStatus) {
        if matches!(self.autonat.status, NatStatus::Public(_))
            && matches!(status, NatStatus::Private)
        {
            self.autonat.last_status_public = true;
        }

        self.autonat.status = status;
    }

    // TODO: Revisit AutoNAT protocol integration
    // pub(crate) fn is_autonat_status_public(&self) -> bool {
    //     matches!(self.autonat.status, NatStatus::Public(_))
    // }

    // pub(crate) fn is_autonat_status_private(&self) -> bool {
    //     matches!(self.autonat.status, NatStatus::Private)
    // }

    // pub(crate) fn autonat_became_private(&self) -> bool {
    //     self.autonat.last_status_public
    // }
}

/// PeerInfo is a struct that holds information about a peer.
/// It offers immutable methods for accessing the information.
#[derive(Clone, Debug, Default)]
pub struct PeerInfo {
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
            .filter(|addr| addr.iter().any(|p| matches!(p, Protocol::Udp(_))))
            .collect();

        match udp_addrs.len() {
            0 => self.addrs.iter().next(),
            _ => Some(udp_addrs[0]),
        }
    }

    pub(crate) fn is_rendezvous_discover_throttled(&self, rpm: f32) -> bool {
        self.rendezvous.as_ref().map_or(false, |info| {
            info.last_discovery_at().map_or(false, |instant| {
                instant.elapsed() < Duration::from_secs_f32(60.0 / rpm)
            })
        })
    }

    pub(crate) const fn rendezvous(&self) -> Option<&PeerRendezvousInfo> {
        self.rendezvous.as_ref()
    }

    pub(crate) const fn relay(&self) -> Option<&PeerRelayInfo> {
        self.relay.as_ref()
    }

    fn add_discovery_mechanism(&mut self, mechanism: PeerDiscoveryMechanism) {
        let _ = self.discoveries.insert(mechanism);
    }

    fn update_rendezvous_cookie(&mut self, cookie: Cookie) {
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
pub enum PeerDiscoveryMechanism {
    Mdns,
    Rendezvous,
}

#[derive(Clone, Debug, Default)]
pub struct PeerRelayInfo {
    reservation_status: RelayReservationStatus,
}

impl PeerRelayInfo {
    pub(crate) const fn reservation_status(&self) -> RelayReservationStatus {
        self.reservation_status
    }

    fn update_reservation_status(&mut self, status: RelayReservationStatus) {
        self.reservation_status = status;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RelayReservationStatus {
    #[default]
    Discovered,
    Requested,
    Accepted,
    Expired,
}

#[derive(Clone, Debug, Default)]
pub struct PeerRendezvousInfo {
    cookie: Option<Cookie>,
    last_discovery_at: Option<Instant>,
    registration_status: RendezvousRegistrationStatus,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RendezvousRegistrationStatus {
    #[default]
    Discovered,
    Requested,
    Registered,
    Expired,
}

impl PeerRendezvousInfo {
    pub(crate) const fn cookie(&self) -> Option<&Cookie> {
        self.cookie.as_ref()
    }

    pub(crate) const fn last_discovery_at(&self) -> Option<Instant> {
        self.last_discovery_at
    }

    fn update_cookie(&mut self, cookie: Cookie) {
        self.cookie = Some(cookie);
        self.last_discovery_at = Some(Instant::now());
    }

    pub(crate) const fn registration_status(&self) -> RendezvousRegistrationStatus {
        self.registration_status
    }

    fn update_registration_status(&mut self, status: RendezvousRegistrationStatus) {
        self.registration_status = status;
    }
}
