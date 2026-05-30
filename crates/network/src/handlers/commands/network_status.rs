//! Handler for `NetworkMessage::NetworkStatus` — snapshots the swarm's
//! current connectivity state for the admin-api consumer.
//!
//! Read-only: walks `self.swarm` and `self.discovery.state` once, copies
//! the live data into owned `NetworkStatusSnapshot` types, and returns.
//! The admin-api wire format (all strings, RFC3339 timestamps) is built
//! one layer up in `crates/server/src/admin/handlers/network/status.rs`.

use std::time::{Instant, SystemTime};

use actix::{Context, Handler, MessageResult};
use calimero_network_primitives::messages::NetworkStatus;
use calimero_network_primitives::network_status::{
    AutonatEntry, AutonatTestEntry, AutonatTestKind, DirectUpgradeEntry, DirectUpgradeOutcome,
    NetworkStatusSnapshot, ReachabilityKind, RelayEntry, RelayReservationKind, RendezvousEntry,
    RendezvousRegistrationKind,
};

use crate::discovery::state::{
    AutonatTestResult, DcutrUpgradeStatus, ReachabilityState, RelayReservationStatus,
    RendezvousRegistrationStatus,
};
use crate::NetworkManager;

impl Handler<NetworkStatus> for NetworkManager {
    // actix's `MessageResponse` is not implemented for arbitrary owned
    // types — only for primitives, wrappers, and `MessageResult<M>`. We
    // wrap our snapshot in `MessageResult` so the response plumbing
    // accepts it; the caller's `oneshot::Receiver` resolves to the
    // unwrapped `NetworkStatusSnapshot`.
    type Result = MessageResult<NetworkStatus>;

    fn handle(&mut self, _msg: NetworkStatus, _ctx: &mut Context<Self>) -> Self::Result {
        // Capture a single (Instant, SystemTime) pair as the conversion
        // anchor. Doing it once means every retained `Instant` is
        // converted against the same wall clock — small consistency win
        // worth the one extra binding.
        let now_instant = Instant::now();
        let now_system = SystemTime::now();
        let to_system = |instant: Instant| -> SystemTime {
            // `Instant` is monotonic and may be slightly in the future
            // relative to `now_instant` if a concurrent recorder bumped
            // it between our two reads. Saturate to `now_system` in
            // that case rather than panicking on subtraction.
            now_instant
                .checked_duration_since(instant)
                .map_or(now_system, |elapsed| now_system - elapsed)
        };

        let local_peer_id = *self.swarm.local_peer_id();
        let listen_addrs = self.swarm.listeners().cloned().collect();
        let external_addrs = self.swarm.external_addresses().cloned().collect();

        let state = &self.discovery.state;

        let mut relays = Vec::new();
        let mut rendezvous = Vec::new();
        let mut direct_upgrades = Vec::new();

        for (peer_id, info) in state.iter_peers() {
            if let Some(relay_info) = info.relay() {
                relays.push(RelayEntry {
                    peer_id: *peer_id,
                    reservation_status: map_relay_status(relay_info.reservation_status()),
                    last_state_change: to_system(relay_info.last_state_change()),
                });
            }
            if let Some(rdv_info) = info.rendezvous() {
                rendezvous.push(RendezvousEntry {
                    peer_id: *peer_id,
                    registration_status: map_rendezvous_status(rdv_info.registration_status()),
                    last_state_change: to_system(rdv_info.last_state_change()),
                });
            }
            if let Some(dcutr_info) = info.dcutr() {
                direct_upgrades.push(DirectUpgradeEntry {
                    peer_id: *peer_id,
                    outcome: map_dcutr_status(dcutr_info.status()),
                    last_attempt: to_system(dcutr_info.at()),
                });
            }
        }

        let autonat = AutonatEntry {
            reachability: map_reachability(state.reachability_state()),
            last_test: state.last_autonat_test().map(|test| AutonatTestEntry {
                tested_addr: test.tested_addr.clone(),
                result: match &test.result {
                    AutonatTestResult::Reachable { addr } => {
                        AutonatTestKind::Reachable { addr: addr.clone() }
                    }
                    AutonatTestResult::Failed { reason } => AutonatTestKind::Failed {
                        reason: reason.clone(),
                    },
                },
                at: to_system(test.at),
            }),
        };

        MessageResult(NetworkStatusSnapshot {
            local_peer_id,
            listen_addrs,
            external_addrs,
            relays,
            rendezvous,
            direct_upgrades,
            autonat,
        })
    }
}

fn map_relay_status(status: RelayReservationStatus) -> RelayReservationKind {
    match status {
        RelayReservationStatus::Discovered => RelayReservationKind::Discovered,
        RelayReservationStatus::Requested => RelayReservationKind::Requested,
        RelayReservationStatus::Accepted => RelayReservationKind::Accepted,
        RelayReservationStatus::Expired => RelayReservationKind::Expired,
    }
}

fn map_rendezvous_status(status: RendezvousRegistrationStatus) -> RendezvousRegistrationKind {
    match status {
        RendezvousRegistrationStatus::Discovered => RendezvousRegistrationKind::Discovered,
        RendezvousRegistrationStatus::Pending => RendezvousRegistrationKind::Pending,
        RendezvousRegistrationStatus::Requested => RendezvousRegistrationKind::Requested,
        RendezvousRegistrationStatus::Registered => RendezvousRegistrationKind::Registered,
        RendezvousRegistrationStatus::Expired => RendezvousRegistrationKind::Expired,
    }
}

fn map_dcutr_status(status: &DcutrUpgradeStatus) -> DirectUpgradeOutcome {
    match status {
        DcutrUpgradeStatus::Succeeded { connection_id } => DirectUpgradeOutcome::Succeeded {
            connection_id: *connection_id,
        },
        DcutrUpgradeStatus::Failed { reason } => DirectUpgradeOutcome::Failed {
            reason: reason.clone(),
        },
    }
}

fn map_reachability(state: ReachabilityState) -> ReachabilityKind {
    match state {
        ReachabilityState::Unknown => ReachabilityKind::Unknown,
        ReachabilityState::Reachable => ReachabilityKind::Public,
        ReachabilityState::Unreachable => ReachabilityKind::Private,
    }
}
