#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

use core::time::Duration;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

/// Consecutive-failure threshold at which an address is evicted from a
/// peer's address book. Three is chosen as a small magic number that
/// tolerates transient flakiness (one bad TCP retransmit, one identify
/// race) without keeping a permanently broken address around long enough
/// to waste many rendezvous-tick dial attempts.
pub(crate) const DIAL_FAILURE_EVICTION_THRESHOLD: u8 = 3;

use libp2p::relay::HOP_PROTOCOL_NAME;
use libp2p::rendezvous::Cookie;
use libp2p::{Multiaddr, PeerId, StreamProtocol};
use multiaddr::Protocol;
use tracing::info;

// The rendezvous protocol name is not public in libp2p, so we have to define it here.
// source: https://github.com/libp2p/rust-libp2p/blob/a8888a7978f08ec9b8762207bf166193bf312b94/protocols/rendezvous/src/lib.rs#L50C12-L50C92
const RENDEZVOUS_PROTOCOL_NAME: StreamProtocol = StreamProtocol::new("/rendezvous/1.0.0");

/// DiscoveryState is a struct that holds the state of the disovered peers.
/// It holds the relay and rendezvous indexes to quickly check if a peer is a relay or rendezvous.
/// It offers mutable methods for managing the state of the peers.
#[derive(Debug)]
pub struct DiscoveryState {
    peers: BTreeMap<PeerId, PeerInfo>,
    relay_index: BTreeSet<PeerId>,
    rendezvous_index: BTreeSet<PeerId>,
    autonat_index: BTreeSet<PeerId>,
    confirmed_external_addresses: HashSet<Multiaddr>,
    reachability_state: ReachabilityState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReachabilityState {
    Unknown,
    Reachable,
    Unreachable,
}

impl Default for DiscoveryState {
    fn default() -> Self {
        Self {
            peers: BTreeMap::new(),
            relay_index: BTreeSet::new(),
            rendezvous_index: BTreeSet::new(),
            autonat_index: BTreeSet::new(),
            confirmed_external_addresses: HashSet::new(),
            reachability_state: ReachabilityState::Unknown,
        }
    }
}

/// Pure data: what actions NetworkManager should execute
#[derive(Debug, Default)]
pub struct ReachabilityActions {
    pub enable_autonat_server: bool,
    pub disable_autonat_server: bool,
    pub rendezvous_register: Vec<PeerId>,
    pub rendezvous_unregister: Vec<PeerId>,
    pub relay_reservations: Vec<PeerId>,
    pub rendezvous_discover: Vec<PeerId>,
}

impl ReachabilityActions {
    pub fn none() -> Self {
        Self::default()
    }

    pub fn has_actions(&self) -> bool {
        self.enable_autonat_server
            || self.disable_autonat_server
            || !self.rendezvous_register.is_empty()
            || !self.rendezvous_unregister.is_empty()
            || !self.relay_reservations.is_empty()
            || !self.rendezvous_discover.is_empty()
    }
}

impl DiscoveryState {
    /// Called when an address is confirmed reachable (AutoNAT or Swarm)
    pub fn on_address_confirmed(&mut self, addr: &Multiaddr) -> ReachabilityActions {
        self.confirmed_external_addresses.insert(addr.clone());
        self.check_transition()
    }

    /// Called when an address is removed/unreachable
    pub fn on_address_removed(&mut self, addr: &Multiaddr) -> ReachabilityActions {
        self.confirmed_external_addresses.remove(addr);
        self.check_transition()
    }

    /// Core state machine: check if transition occurred and return actions
    fn check_transition(&mut self) -> ReachabilityActions {
        let has_confirmed = self.has_confirmed_external_addresses();
        let current = self.reachability_state;

        let new_state = if has_confirmed {
            ReachabilityState::Reachable
        } else if current == ReachabilityState::Unknown {
            // First failure means we're unreachable
            ReachabilityState::Unreachable
        } else {
            ReachabilityState::Unreachable
        };

        // Only act on actual transitions
        if current == new_state {
            return ReachabilityActions::none();
        }

        info!("Reachability: {:?} → {:?}", current, new_state);
        self.reachability_state = new_state;

        match new_state {
            ReachabilityState::Reachable => self.became_reachable(),
            ReachabilityState::Unreachable => self.became_unreachable(),
            ReachabilityState::Unknown => ReachabilityActions::none(),
        }
    }

    fn became_reachable(&self) -> ReachabilityActions {
        info!("🌐 Node is now publicly reachable");

        ReachabilityActions {
            enable_autonat_server: true,
            disable_autonat_server: false,
            rendezvous_register: self.get_rendezvous_peer_ids().collect(),
            rendezvous_unregister: vec![],
            relay_reservations: vec![],
            rendezvous_discover: vec![],
        }
    }

    fn became_unreachable(&self) -> ReachabilityActions {
        info!("🔒 Node is behind NAT, not publicly reachable");

        let rendezvous_peers: Vec<_> = self.get_rendezvous_peer_ids().collect();
        let relay_peers: Vec<_> = self.get_relay_peer_ids().collect();

        ReachabilityActions {
            enable_autonat_server: false,
            disable_autonat_server: true,
            rendezvous_register: rendezvous_peers.clone(),
            rendezvous_unregister: rendezvous_peers.clone(),
            relay_reservations: relay_peers,
            rendezvous_discover: rendezvous_peers,
        }
    }

    /// Record an address for a peer, or reset its failure counter to zero if
    /// it already exists. Called from successful connection events (the
    /// address obviously works), from identify (the peer told us it
    /// listens there), and from the rendezvous discovery path.
    ///
    /// Addresses are stored as supplied, without normalization. The caller
    /// is responsible for filtering out forms we don't want to dial
    /// directly — most notably relayed multiaddrs (`/p2p-circuit/`) for
    /// inbound connection records.
    pub(crate) fn add_peer_addr(&mut self, peer_id: PeerId, addr: &Multiaddr) {
        let _ = self
            .peers
            .entry(peer_id)
            .or_default()
            .addrs
            .insert(addr.clone(), 0);
    }

    /// Mark a dial failure for `addr` under `peer_id`. Increments the
    /// per-address counter; if it reaches
    /// [`DIAL_FAILURE_EVICTION_THRESHOLD`], evicts the address and returns
    /// true. No-op if the peer or address is not in the book (we don't
    /// add entries for addresses we never planned to keep).
    pub(crate) fn record_dial_failure(&mut self, peer_id: &PeerId, addr: &Multiaddr) -> bool {
        let Some(peer_info) = self.peers.get_mut(peer_id) else {
            return false;
        };
        let Some(count) = peer_info.addrs.get_mut(addr) else {
            return false;
        };

        *count = count.saturating_add(1);
        if *count >= DIAL_FAILURE_EVICTION_THRESHOLD {
            let _ = peer_info.addrs.remove(addr);
            true
        } else {
            false
        }
    }

    pub(crate) fn remove_peer(&mut self, peer_id: &PeerId) {
        drop(self.peers.remove(peer_id));
        let _ = self.relay_index.remove(peer_id);
        let _ = self.rendezvous_index.remove(peer_id);
        let _ = self.autonat_index.remove(peer_id);
    }

    pub(crate) fn update_peer_protocols(&mut self, peer_id: &PeerId, protocols: &[StreamProtocol]) {
        for protocol in protocols {
            if protocol == &HOP_PROTOCOL_NAME {
                let _ = self.relay_index.insert(*peer_id);

                let peer_info = self.peers.entry(*peer_id).or_default();
                let _ignored = peer_info.relay.get_or_insert_with(Default::default);
            }
            if protocol == &RENDEZVOUS_PROTOCOL_NAME {
                let _ = self.rendezvous_index.insert(*peer_id);

                let peer_info = self.peers.entry(*peer_id).or_default();
                let _ignored = peer_info.rendezvous.get_or_insert_with(Default::default);
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
            .is_some_and(|info| info.discoveries.contains(&mechanism))
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
                    addrs: HashMap::default(),
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

    /// Called when a relay reservation is lost — relayed listen address
    /// expired, listener closed, or the control connection to the relay
    /// dropped.
    ///
    /// A single disconnect typically produces several of these events in
    /// quick succession (ConnectionClosed, then ListenerClosed for the dead
    /// listener, then ExternalAddrExpired for the dead address). The first
    /// one finds the peer in `Accepted`, marks `Expired`, and queues
    /// recovery; the downstream call to `create_relay_reservation` flips
    /// status to `Requested` and starts a new libp2p listener. Without
    /// further care, the next event in the burst would see `Requested`,
    /// treat it as "active", and queue another `listen_on`, producing
    /// duplicate listeners (and looping in the quota-denial case).
    ///
    /// To prevent that, only the `Accepted -> Expired` transition queues a
    /// recovery. From `Requested` we still flip to `Expired` (state stays
    /// authoritative), but we do not queue: either the request itself
    /// failed (queuing would loop on a deliberate denial) or this is a
    /// stale event from a prior disconnect whose recovery is already in
    /// flight (queuing would duplicate it). The in-flight libp2p listener
    /// is untouched and, when its reservation completes, ExternalAddrConfirmed
    /// will set status back to `Accepted`.
    ///
    /// The downstream [`crate::NetworkManager::create_relay_reservation`]
    /// still gates on the configured registrations limit, so this only
    /// enqueues intent; it does not unconditionally dial.
    pub(crate) fn on_relay_reservation_lost(&mut self, relay_peer: &PeerId) -> ReachabilityActions {
        let prior_status = self
            .get_peer_info(relay_peer)
            .and_then(|info| info.relay())
            .map(|info| info.reservation_status());

        match prior_status {
            // Lost an Accepted reservation — the case recovery is for.
            Some(RelayReservationStatus::Accepted) => {
                self.update_relay_reservation_status(relay_peer, RelayReservationStatus::Expired);

                if !self.relay_index.contains(relay_peer) {
                    return ReachabilityActions::none();
                }

                ReachabilityActions {
                    relay_reservations: vec![*relay_peer],
                    ..ReachabilityActions::none()
                }
            }
            // Pending request failed, or stale event for a prior loss whose
            // recovery is in flight. Mark Expired but do not queue.
            Some(RelayReservationStatus::Requested) => {
                self.update_relay_reservation_status(relay_peer, RelayReservationStatus::Expired);
                ReachabilityActions::none()
            }
            // Already-Expired or never-tracked peer. Nothing to do.
            Some(RelayReservationStatus::Expired | RelayReservationStatus::Discovered) | None => {
                ReachabilityActions::none()
            }
        }
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

    pub(crate) fn has_confirmed_external_addresses(&self) -> bool {
        !self.confirmed_external_addresses.is_empty()
    }

    pub(crate) fn add_autonat_server(&mut self, peer_id: &PeerId) {
        _ = self.autonat_index.insert(*peer_id);
        _ = self.peers.entry(*peer_id).or_default();
    }

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
}

/// PeerInfo is a struct that holds information about a peer.
/// It offers immutable methods for accessing the information.
///
/// `addrs` maps each known address to the number of consecutive dial
/// failures observed for it. The counter is reset on every successful
/// connection (or on a fresh identify push that re-introduces the
/// address) and incremented on `OutgoingConnectionError`. An address is
/// evicted entirely once the count reaches
/// [`DIAL_FAILURE_EVICTION_THRESHOLD`]. This bounds growth without
/// penalising stable long-online peers — a working address keeps its
/// counter at zero indefinitely.
#[derive(Clone, Debug, Default)]
pub struct PeerInfo {
    addrs: HashMap<Multiaddr, u8>,
    discoveries: HashSet<PeerDiscoveryMechanism>,
    relay: Option<PeerRelayInfo>,
    rendezvous: Option<PeerRendezvousInfo>,
}

impl PeerInfo {
    pub(crate) fn addrs(&self) -> impl Iterator<Item = &Multiaddr> {
        self.addrs.keys()
    }

    pub(crate) fn get_preferred_addr(&self) -> Option<&Multiaddr> {
        let udp_addrs: Vec<&Multiaddr> = self
            .addrs
            .keys()
            .filter(|addr| addr.iter().any(|p| matches!(p, Protocol::Udp(_))))
            .collect();

        match udp_addrs.len() {
            0 => self.addrs.keys().next(),
            _ => Some(udp_addrs[0]),
        }
    }

    pub(crate) fn is_rendezvous_discover_throttled(&self, rpm: f32) -> bool {
        self.rendezvous.as_ref().is_some_and(|info| {
            info.last_discovery_at()
                .is_some_and(|instant| instant.elapsed() < Duration::from_secs_f32(60.0 / rpm))
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

    const fn update_relay_reservation_status(&mut self, status: RelayReservationStatus) {
        if let Some(ref mut info) = self.relay {
            info.update_reservation_status(status);
        }
    }

    const fn update_rendezvous_registartion_status(
        &mut self,
        status: RendezvousRegistrationStatus,
    ) {
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

    const fn update_reservation_status(&mut self, status: RelayReservationStatus) {
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

    const fn update_registration_status(&mut self, status: RendezvousRegistrationStatus) {
        self.registration_status = status;
    }
}
