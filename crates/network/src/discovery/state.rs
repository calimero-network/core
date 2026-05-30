#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

use core::time::Duration;
use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

use libp2p::core::transport::ListenerId;
use libp2p::relay::HOP_PROTOCOL_NAME;
use libp2p::rendezvous::{Cookie, Namespace};
use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId, StreamProtocol};
use multiaddr::Protocol;
use tracing::info;

// The rendezvous protocol name is not public in libp2p, so we have to define it here.
// source: https://github.com/libp2p/rust-libp2p/blob/a8888a7978f08ec9b8762207bf166193bf312b94/protocols/rendezvous/src/lib.rs#L50C12-L50C92
const RENDEZVOUS_PROTOCOL_NAME: StreamProtocol = StreamProtocol::new("/rendezvous/1.0.0");

/// Consecutive-failure threshold at which an address is evicted from a
/// peer's address book. Three is chosen as a small magic number that
/// tolerates transient flakiness (one bad TCP retransmit, one identify
/// race) without keeping a permanently broken address around long enough
/// to waste many rendezvous-tick dial attempts.
pub(crate) const DIAL_FAILURE_EVICTION_THRESHOLD: u8 = 3;

/// Rendezvous-key prefixes for per-overlay registration/discovery.
///
/// Instead of one global rendezvous namespace (which returns every node
/// on the network), a node registers and discovers under one key per
/// overlay topic it follows. `discover` then returns only co-members of
/// that exact namespace/group/context — relevant peers by construction,
/// so we never fan out relayed dials at the whole network and never
/// saturate the relay client's in-flight circuit cap.
///
/// The key is derived deterministically from the gossipsub topic string,
/// so a registering member and a discovering peer that holds the same id
/// (e.g. a joiner that learned `namespace_id` from an invite) compute the
/// identical key without any extra coordination.
const RENDEZVOUS_NS_PREFIX: &str = "/calimero/ns/";
const RENDEZVOUS_GRP_PREFIX: &str = "/calimero/grp/";
const RENDEZVOUS_CTX_PREFIX: &str = "/calimero/ctx/";

/// Map a subscribed gossipsub topic to its rendezvous key.
///
/// - `ns/<hex>`     → `/calimero/ns/<hex>`
/// - `group/<hex>`  → `/calimero/grp/<hex>`
/// - bare `<id>`    → `/calimero/ctx/<id>` (a context-id topic)
///
/// The two structured topics already carry hex ids; a bare topic is a
/// context id (the network layer treats it opaquely — both sides hold
/// the identical context-topic string, so the derived keys match).
///
/// Returns `None` only when the resulting key exceeds the rendezvous
/// 255-char namespace limit. Callers are responsible for not passing
/// non-overlay topics (e.g. the configured specialized-node invite
/// topic) — those are filtered upstream where the config is known.
pub(crate) fn rendezvous_key_for_topic(topic: &str) -> Option<Namespace> {
    let raw = if let Some(id) = topic.strip_prefix("ns/") {
        format!("{RENDEZVOUS_NS_PREFIX}{id}")
    } else if let Some(id) = topic.strip_prefix("group/") {
        format!("{RENDEZVOUS_GRP_PREFIX}{id}")
    } else {
        format!("{RENDEZVOUS_CTX_PREFIX}{topic}")
    };
    // `Namespace::new` rejects keys over `MAX_NAMESPACE` (255). All our
    // ids are <= 64 hex / ~44 bs58 chars plus a short prefix, so this
    // only trips on a pathological topic; drop it rather than panic.
    Namespace::new(raw).ok()
}

/// Given the node's subscribed topics paired with their current
/// connected-**subscriber** counts, return the rendezvous keys we are
/// *under-connected* on — i.e. topics with zero connected subscribers.
///
/// The count is the full subscriber set (gossipsub `all_peers`), NOT the
/// grafted mesh: a topic with a connected subscriber can sync through it
/// even when the mesh is momentarily thin, so it isn't under-connected.
///
/// These are the only keys worth spending a (paced) rendezvous discover
/// on: a topic that already has a connected co-member can sync through
/// it, so re-discovering would just add rendezvous load. This generalises
/// the node-wide [`DiscoveryState::has_regular_connected_peer`] gate to
/// per-overlay granularity, so discovery cost scales with how many of our
/// collaborations currently lack peers (≈0 in steady state, spiking only
/// after a restart/partition — exactly when we want it) rather than with
/// total membership.
///
/// Duplicate keys (e.g. two malformed topics that map alike) are
/// collapsed; order follows first appearance in `topics`.
pub(crate) fn under_connected_rendezvous_keys<'a>(
    topics: impl IntoIterator<Item = (&'a str, usize)>,
) -> Vec<Namespace> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for (topic, subscriber_count) in topics {
        if subscriber_count > 0 {
            continue;
        }
        if let Some(key) = rendezvous_key_for_topic(topic) {
            if seen.insert(key.clone()) {
                keys.push(key);
            }
        }
    }
    keys
}

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
    /// Maps each libp2p relayed listener back to the relay peer it was
    /// opened against. Populated by `create_relay_reservation` when it
    /// calls `listen_on(<relay>/p2p-circuit/<self>)` and gets back a
    /// `ListenerId`. Looked up by the `ListenerClosed` swarm handler so
    /// it can route the recovery action even when the closed listener's
    /// `addresses` list is empty (e.g. quota denial before address
    /// allocation).
    relay_listeners: HashMap<ListenerId, PeerId>,
    reachability_state: ReachabilityState,
    /// Most recent AutoNAT v2 client probe outcome. Overwritten on every
    /// probe; `None` until the first probe lands. Surfaced via
    /// `meroctl network status` so operators can answer "is this node
    /// behind NAT?" without trawling `RUST_LOG=debug`.
    last_autonat_test: Option<AutonatTest>,
}

/// Latest AutoNAT client test outcome retained for introspection.
/// One slot only — we don't keep history; the value is the freshest
/// observation. The address tested and the result are both reported.
#[derive(Clone, Debug)]
pub struct AutonatTest {
    pub tested_addr: Multiaddr,
    pub result: AutonatTestResult,
    pub at: Instant,
}

#[derive(Clone, Debug)]
pub enum AutonatTestResult {
    Reachable { addr: Multiaddr },
    Failed { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachabilityState {
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
            relay_listeners: HashMap::new(),
            reachability_state: ReachabilityState::Unknown,
            last_autonat_test: None,
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
    /// Throttled rendezvous-discovery queue. The periodic
    /// `discovery_interval` tick and steady-state reachability
    /// transitions populate this; entries are gated by the
    /// per-rendezvous-peer `discovery_rpm` throttle so a node
    /// doesn't hammer the rendezvous server.
    pub rendezvous_discover: Vec<PeerId>,
    /// Event-driven rendezvous re-discovery that bypasses the
    /// `discovery_rpm` throttle. Used by paths where the throttle
    /// floor (default 120s with rpm=0.5) is much longer than the
    /// fault-recovery budget — most notably, losing the last
    /// connection to a regular (non-rendezvous, non-relay) peer.
    /// If we wait for the next throttled tick we miss the post-
    /// restart peer's fresh registration and the upstream
    /// `crates/node` sync layer parks on "No peers to sync with"
    /// for the rest of the test budget (issue #2469 under NAT).
    pub rendezvous_discover_force: Vec<PeerId>,
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
            || !self.rendezvous_discover_force.is_empty()
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
            rendezvous_discover_force: vec![],
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
            rendezvous_discover_force: vec![],
        }
    }

    /// Called from `SwarmEvent::ConnectionClosed` when the last
    /// connection to a regular peer (not a relay, not a rendezvous
    /// peer, not an mdns-discovered peer) drops.
    ///
    /// The post-`docker restart` / post-pause / post-partition
    /// scenarios all hit the same shape: peer A's connection to
    /// peer B closes; B comes back with a fresh libp2p identity
    /// state (new relay reservation, new `/p2p-circuit/`
    /// multiaddr) and re-registers via rendezvous on the
    /// boot-node; A has the boot-node connection but is throttled
    /// out of querying rendezvous for ~120s (default
    /// `discovery_rpm` is 0.5 = 1 query / 2 minutes), so A
    /// doesn't pick up B's fresh registration until the throttle
    /// expires. With a 120s sync-recovery budget, A never
    /// rediscovers B and the upstream sync layer parks on
    /// "No peers to sync with" indefinitely (issue #2469).
    ///
    /// Returns `rendezvous_discover_force` for every known
    /// rendezvous peer, which `execute_reachability_actions`
    /// dispatches via the force-path (throttle bypassed).
    /// Empty-action no-op if we have no rendezvous peers to
    /// query (e.g. local-only mdns discovery).
    pub(crate) fn on_regular_peer_disconnected(&self) -> ReachabilityActions {
        let rendezvous_peers: Vec<_> = self.get_rendezvous_peer_ids().collect();
        if rendezvous_peers.is_empty() {
            return ReachabilityActions::none();
        }
        ReachabilityActions {
            rendezvous_discover_force: rendezvous_peers,
            ..ReachabilityActions::none()
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
                    dcutr: None,
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

    /// Remember that `listener_id` was opened against `relay_peer` as a
    /// relayed listener. The ListenerClosed handler uses this to route
    /// recovery for the quota-denied case where libp2p tears the listener
    /// down before any external address is ever attached — leaving
    /// `addresses` empty in the event and the address-iteration fallback
    /// with nothing to act on.
    pub(crate) fn record_relay_listener(&mut self, listener_id: ListenerId, relay_peer: PeerId) {
        let _ = self.relay_listeners.insert(listener_id, relay_peer);
    }

    /// Remove and return the relay peer associated with a libp2p listener
    /// id. Returns `None` if the listener wasn't registered (a non-relay
    /// TCP/QUIC listener, or a relayed listener opened outside
    /// `create_relay_reservation`).
    ///
    /// Combines lookup and cleanup in one call so the caller cannot
    /// accidentally leave a stale entry behind on any code path. This
    /// matters because the `ListenerClosed` handler falls through to an
    /// addresses-iteration fallback when the lookup misses; a
    /// lookup-then-conditional-forget shape would leak entries for
    /// listeners that were registered but somehow took the fallback
    /// path. With `take_relay_listener`, the map mutation always happens.
    pub(crate) fn take_relay_listener(&mut self, listener_id: &ListenerId) -> Option<PeerId> {
        self.relay_listeners.remove(listener_id)
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

    /// Returns `true` if any peer in `connected` is a "regular" peer —
    /// i.e. neither a relay nor a rendezvous infrastructure peer.
    ///
    /// A NAT'd node can hold connections to its relay and rendezvous
    /// servers while having zero connections to application peers; in
    /// that state it is effectively partitioned from the overlay even
    /// though `connected_peers()` is non-empty. The rendezvous tick uses
    /// this to decide whether to bypass the `discovery_rpm` throttle and
    /// re-find peers aggressively.
    ///
    /// This is the post-restart analogue of the #2469
    /// `on_regular_peer_disconnected` force-rediscovery: that path is
    /// keyed on `SwarmEvent::ConnectionClosed`, which never fires after a
    /// fresh restart (no connection was ever open this process), so a
    /// just-restarted node would otherwise sit behind the 120s throttle
    /// floor while the upstream sync layer parks on "No peers to sync
    /// with".
    pub(crate) fn has_regular_connected_peer<'a>(
        &self,
        connected: impl IntoIterator<Item = &'a PeerId>,
    ) -> bool {
        connected.into_iter().any(|peer| {
            // All three infra indices are excluded: a node whose only
            // connection is to a relay, a rendezvous server, OR a
            // dedicated autonat server is still partitioned from the
            // application overlay and must keep force-rediscovering.
            !self.relay_index.contains(peer)
                && !self.rendezvous_index.contains(peer)
                && !self.autonat_index.contains(peer)
        })
    }

    pub(crate) fn has_confirmed_external_addresses(&self) -> bool {
        !self.confirmed_external_addresses.is_empty()
    }

    pub(crate) fn add_autonat_server(&mut self, peer_id: &PeerId) {
        _ = self.autonat_index.insert(*peer_id);
        _ = self.peers.entry(*peer_id).or_default();
    }

    /// Record the outcome of a DCUtR hole-punch attempt with `peer_id`.
    /// Ensures the peer exists in the registry first — a dcutr event can
    /// fire for a peer we don't otherwise track (no identify yet, no
    /// rendezvous discovery), and we still want to surface the result.
    pub(crate) fn record_dcutr_outcome(&mut self, peer_id: PeerId, status: DcutrUpgradeStatus) {
        self.peers.entry(peer_id).or_default().update_dcutr(status);
    }

    /// Record the outcome of the most recent AutoNAT client probe.
    /// Overwrites any prior observation — callers wanting history should
    /// read `last_autonat_test` between calls.
    pub(crate) fn record_autonat_test(
        &mut self,
        tested_addr: Multiaddr,
        result: AutonatTestResult,
    ) {
        self.last_autonat_test = Some(AutonatTest {
            tested_addr,
            result,
            at: Instant::now(),
        });
    }

    pub(crate) fn last_autonat_test(&self) -> Option<&AutonatTest> {
        self.last_autonat_test.as_ref()
    }

    pub(crate) const fn reachability_state(&self) -> ReachabilityState {
        self.reachability_state
    }

    /// Iterate over `(peer_id, info)` pairs for every peer we track.
    /// Used by the network-status snapshot builder to enumerate relay,
    /// rendezvous and dcutr state in one pass.
    pub(crate) fn iter_peers(&self) -> impl Iterator<Item = (&PeerId, &PeerInfo)> {
        self.peers.iter()
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
                        // `Pending` is not a real registration — it doesn't
                        // occupy a slot, so the fan-out gate keeps re-attempting
                        // until the registration actually lands.
                        RendezvousRegistrationStatus::Discovered
                        | RendezvousRegistrationStatus::Pending
                        | RendezvousRegistrationStatus::Expired => acc,
                    }
                })
            });
        sum < max
    }

    /// Nominate a rendezvous peer to (re)register with.
    ///
    /// Two priority tiers:
    /// - **Eager (first match wins, returned immediately):** `Discovered`
    ///   (never tried) and `Pending` (tried, waiting on an external
    ///   address). These are *equal* priority — whichever the (PeerId-
    ///   sorted) iteration reaches first is taken. There is deliberately
    ///   no preference between them: `Pending` is not a failed peer, just
    ///   one blocked on a transient missing-external-address condition, so
    ///   there's no reason to deprioritize re-attempting it over a fresh
    ///   `Discovered` peer.
    /// - **Fallback:** an `Expired` peer, used only if no eager peer
    ///   exists.
    ///
    /// Returns `None` when every known rendezvous peer is already
    /// `Requested`/`Registered`.
    ///
    /// `Pending` must be nominated here, not skipped: when a slot frees
    /// (e.g. another peer's registration `Expired`), this is the path that
    /// re-drives a peer blocked on a missing external address. Skipping it
    /// would strand that peer until the next `ExternalAddrConfirmed` —
    /// which is exactly the slot it occupied as `Discovered` before the
    /// `Pending` state existed. An eager peer wins over an already-found
    /// `Expired` candidate via early return, independent of iteration
    /// order.
    pub(crate) fn find_new_rendezvous_peer(&self) -> Option<PeerId> {
        let mut candidate = None;

        for peer_id in self.get_rendezvous_peer_ids() {
            let Some(peer_info) = self.get_peer_info(&peer_id) else {
                continue;
            };
            let Some(rendezvous_info) = peer_info.rendezvous() else {
                continue;
            };
            match rendezvous_info.registration_status() {
                RendezvousRegistrationStatus::Discovered
                | RendezvousRegistrationStatus::Pending => {
                    // Registerable target holding no slot — take it now.
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

        candidate
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
    /// Latest DCUtR hole-punch outcome observed for this peer. `None`
    /// means we've never seen a dcutr event (either the peer isn't
    /// reachable over a relay we attempted to upgrade, or the upgrade
    /// hasn't happened yet). Populated by the dcutr swarm-event handler.
    dcutr: Option<PeerDcutrInfo>,
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

    pub(crate) const fn dcutr(&self) -> Option<&PeerDcutrInfo> {
        self.dcutr.as_ref()
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

    fn update_dcutr(&mut self, status: DcutrUpgradeStatus) {
        self.dcutr = Some(PeerDcutrInfo::new(status));
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PeerDiscoveryMechanism {
    Mdns,
    Rendezvous,
}

#[derive(Clone, Debug)]
pub struct PeerRelayInfo {
    reservation_status: RelayReservationStatus,
    /// Wall-clock instant of the most recent reservation status mutation,
    /// or of struct creation if no mutation has occurred. Surfaced as
    /// `last_state_change` in the network-status snapshot so operators
    /// can tell a fresh transition from a stale one.
    last_state_change: Instant,
}

impl Default for PeerRelayInfo {
    fn default() -> Self {
        Self {
            reservation_status: RelayReservationStatus::default(),
            last_state_change: Instant::now(),
        }
    }
}

impl PeerRelayInfo {
    pub(crate) const fn reservation_status(&self) -> RelayReservationStatus {
        self.reservation_status
    }

    pub(crate) const fn last_state_change(&self) -> Instant {
        self.last_state_change
    }

    fn update_reservation_status(&mut self, status: RelayReservationStatus) {
        self.reservation_status = status;
        self.last_state_change = Instant::now();
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

#[derive(Clone, Debug)]
pub struct PeerRendezvousInfo {
    cookie: Option<Cookie>,
    last_discovery_at: Option<Instant>,
    registration_status: RendezvousRegistrationStatus,
    /// Wall-clock instant of the most recent registration status
    /// mutation. Surfaced as `last_registered_at` in the network-status
    /// snapshot (the name follows the issue spec — it reads as "last
    /// time registration status changed", which is what an operator
    /// debugging rendezvous churn cares about).
    last_state_change: Instant,
}

impl Default for PeerRendezvousInfo {
    fn default() -> Self {
        Self {
            cookie: None,
            last_discovery_at: None,
            registration_status: RendezvousRegistrationStatus::default(),
            last_state_change: Instant::now(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RendezvousRegistrationStatus {
    #[default]
    Discovered,
    /// We attempted to register but the swarm had no external address to
    /// advertise yet, so nothing was actually sent. The registration is
    /// queued and re-attempted on the next `ExternalAddrConfirmed`. Distinct
    /// from `Discovered` (never tried) so observability reflects the truth:
    /// engaged, waiting on an external address.
    Pending,
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

    pub(crate) const fn last_state_change(&self) -> Instant {
        self.last_state_change
    }

    fn update_registration_status(&mut self, status: RendezvousRegistrationStatus) {
        self.registration_status = status;
        self.last_state_change = Instant::now();
    }
}

/// DCUtR (Direct Connection Upgrade through Relay) hole-punch outcome
/// retained per peer. We keep only the latest observation — a failed
/// upgrade followed by a successful retry should leave the peer in the
/// `Succeeded` state. Populated by the dcutr swarm-event handler.
#[derive(Clone, Debug)]
pub struct PeerDcutrInfo {
    status: DcutrUpgradeStatus,
    at: Instant,
}

impl PeerDcutrInfo {
    pub(crate) fn new(status: DcutrUpgradeStatus) -> Self {
        Self {
            status,
            at: Instant::now(),
        }
    }

    pub(crate) fn status(&self) -> &DcutrUpgradeStatus {
        &self.status
    }

    pub(crate) const fn at(&self) -> Instant {
        self.at
    }
}

#[derive(Clone, Debug)]
pub enum DcutrUpgradeStatus {
    Succeeded { connection_id: ConnectionId },
    Failed { reason: String },
}
