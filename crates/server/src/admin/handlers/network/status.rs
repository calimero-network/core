//! `GET /admin-api/network/status` — snapshot of libp2p connectivity.
//!
//! Asks the network actor for a `NetworkStatusSnapshot` and renders it
//! into the wire-stable `NetworkStatusResponse` (all strings, RFC3339
//! UTC timestamps). The rich types stay on the actor side; this layer
//! exists to keep the HTTP contract from drifting whenever libp2p
//! changes the shape of a `PeerId`, `Multiaddr`, or `ConnectionId`.

use std::sync::Arc;
use std::time::SystemTime;

use axum::response::IntoResponse;
use axum::Extension;
use calimero_network_primitives::network_status::{
    AutonatTestKind, DirectUpgradeOutcome, NetworkStatusSnapshot, ReachabilityKind,
    RelayReservationKind, RendezvousRegistrationKind,
};
use calimero_server_primitives::admin::{
    AutonatStatusEntry, DirectUpgradeStatusEntry, NetworkStatusResponse, RelayStatusEntry,
    RendezvousStatusEntry,
};
use chrono::{DateTime, SecondsFormat, Utc};

use crate::admin::service::ApiResponse;
use crate::AdminState;

pub async fn handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    let snapshot = state.node_client.network_status().await;
    ApiResponse {
        payload: render(snapshot),
    }
    .into_response()
}

/// Pure conversion: snapshot → wire response. Extracted so tests can
/// drive it without spinning up a network actor.
pub fn render(snapshot: NetworkStatusSnapshot) -> NetworkStatusResponse {
    NetworkStatusResponse {
        local_peer_id: snapshot.local_peer_id.to_string(),
        listen_addrs: snapshot
            .listen_addrs
            .into_iter()
            .map(|a| a.to_string())
            .collect(),
        external_addrs: snapshot
            .external_addrs
            .into_iter()
            .map(|a| a.to_string())
            .collect(),
        relays: snapshot
            .relays
            .into_iter()
            .map(|r| RelayStatusEntry {
                peer_id: r.peer_id.to_string(),
                reservation_status: relay_status_str(r.reservation_status).to_owned(),
                last_state_change: rfc3339(r.last_state_change),
            })
            .collect(),
        rendezvous: snapshot
            .rendezvous
            .into_iter()
            .map(|r| RendezvousStatusEntry {
                peer_id: r.peer_id.to_string(),
                registration_status: rendezvous_status_str(r.registration_status).to_owned(),
                last_state_change: rfc3339(r.last_state_change),
            })
            .collect(),
        direct_upgrades: snapshot
            .direct_upgrades
            .into_iter()
            .map(|d| {
                let (status, reason, connection_id) = match d.outcome {
                    DirectUpgradeOutcome::Succeeded { connection_id } => (
                        "succeeded".to_owned(),
                        None,
                        Some(format!("{connection_id:?}")),
                    ),
                    DirectUpgradeOutcome::Failed { reason } => {
                        ("failed".to_owned(), Some(reason), None)
                    }
                };
                DirectUpgradeStatusEntry {
                    peer_id: d.peer_id.to_string(),
                    status,
                    reason,
                    connection_id,
                    last_attempt: rfc3339(d.last_attempt),
                }
            })
            .collect(),
        autonat: {
            let reachability = reachability_str(snapshot.autonat.reachability).to_owned();
            let (
                last_test_addr,
                last_test_result,
                last_test_reason,
                last_test_observed_addr,
                last_test_at,
            ) = match snapshot.autonat.last_test {
                None => (None, None, None, None, None),
                Some(test) => {
                    let (result, reason, observed) = match test.result {
                        AutonatTestKind::Reachable { addr } => {
                            ("reachable".to_owned(), None, Some(addr.to_string()))
                        }
                        AutonatTestKind::Failed { reason } => {
                            ("failed".to_owned(), Some(reason), None)
                        }
                    };
                    (
                        Some(test.tested_addr.to_string()),
                        Some(result),
                        reason,
                        observed,
                        Some(rfc3339(test.at)),
                    )
                }
            };
            AutonatStatusEntry {
                reachability,
                last_test_addr,
                last_test_result,
                last_test_reason,
                last_test_observed_addr,
                last_test_at,
            }
        },
    }
}

fn rfc3339(ts: SystemTime) -> String {
    DateTime::<Utc>::from(ts).to_rfc3339_opts(SecondsFormat::Secs, true)
}

const fn relay_status_str(kind: RelayReservationKind) -> &'static str {
    match kind {
        RelayReservationKind::Discovered => "discovered",
        RelayReservationKind::Requested => "requested",
        RelayReservationKind::Accepted => "accepted",
        RelayReservationKind::Expired => "expired",
    }
}

const fn rendezvous_status_str(kind: RendezvousRegistrationKind) -> &'static str {
    match kind {
        RendezvousRegistrationKind::Discovered => "discovered",
        RendezvousRegistrationKind::Requested => "requested",
        RendezvousRegistrationKind::Registered => "registered",
        RendezvousRegistrationKind::Expired => "expired",
    }
}

const fn reachability_str(kind: ReachabilityKind) -> &'static str {
    match kind {
        ReachabilityKind::Unknown => "unknown",
        ReachabilityKind::Public => "public",
        ReachabilityKind::Private => "private",
    }
}
