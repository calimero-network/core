//! Snapshot types for the `NetworkStatus` actor query — the data the
//! `NetworkManager` returns when asked "what does your swarm look like
//! right now?". Consumed by `GET /admin-api/network/status` (which
//! converts to a wire-friendly all-strings shape) and by anything else
//! that wants a typed connectivity view without subscribing to events.
//!
//! Timestamps are `SystemTime` rather than `Instant` because the
//! consumer is an HTTP endpoint that ultimately wants a wall-clock
//! string. We do the `Instant → SystemTime` conversion inside the
//! handler so callers don't have to carry an `Instant` reference clock.

use std::time::SystemTime;

use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId};

/// Snapshot of the local node's libp2p connectivity state.
///
/// Returned in-process by `NetworkMessage::NetworkStatus`. The
/// admin-api handler renders this into JSON for operators; tests can
/// consume it directly without touching HTTP.
#[derive(Clone, Debug)]
pub struct NetworkStatusSnapshot {
    pub local_peer_id: PeerId,
    pub listen_addrs: Vec<Multiaddr>,
    pub external_addrs: Vec<Multiaddr>,
    pub relays: Vec<RelayEntry>,
    pub rendezvous: Vec<RendezvousEntry>,
    pub direct_upgrades: Vec<DirectUpgradeEntry>,
    pub autonat: AutonatEntry,
}

#[derive(Clone, Debug)]
pub struct RelayEntry {
    pub peer_id: PeerId,
    pub reservation_status: RelayReservationKind,
    pub last_state_change: SystemTime,
}

/// Mirrors `calimero_network::discovery::state::RelayReservationStatus`
/// but lives in primitives so the wire crate can name it without
/// pulling in the network behaviour crate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelayReservationKind {
    Discovered,
    Requested,
    Accepted,
    Expired,
}

#[derive(Clone, Debug)]
pub struct RendezvousEntry {
    pub peer_id: PeerId,
    pub registration_status: RendezvousRegistrationKind,
    pub last_state_change: SystemTime,
}

/// Mirrors `calimero_network::discovery::state::RendezvousRegistrationStatus`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendezvousRegistrationKind {
    Discovered,
    Requested,
    Registered,
    Expired,
}

#[derive(Clone, Debug)]
pub struct DirectUpgradeEntry {
    pub peer_id: PeerId,
    pub outcome: DirectUpgradeOutcome,
    pub last_attempt: SystemTime,
}

#[derive(Clone, Debug)]
pub enum DirectUpgradeOutcome {
    Succeeded { connection_id: ConnectionId },
    Failed { reason: String },
}

#[derive(Clone, Debug)]
pub struct AutonatEntry {
    pub reachability: ReachabilityKind,
    pub last_test: Option<AutonatTestEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReachabilityKind {
    Unknown,
    Public,
    Private,
}

#[derive(Clone, Debug)]
pub struct AutonatTestEntry {
    pub tested_addr: Multiaddr,
    pub result: AutonatTestKind,
    pub at: SystemTime,
}

#[derive(Clone, Debug)]
pub enum AutonatTestKind {
    Reachable { addr: Multiaddr },
    Failed { reason: String },
}
