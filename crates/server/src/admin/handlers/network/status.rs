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
                    // `Display` on libp2p's `ConnectionId` writes the bare
                    // integer; `Debug` would write `ConnectionId(N)` and
                    // could change shape across libp2p versions. Keep the
                    // wire stable.
                    DirectUpgradeOutcome::Succeeded { connection_id } => (
                        "succeeded".to_owned(),
                        None,
                        Some(connection_id.to_string()),
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
        RendezvousRegistrationKind::Pending => "pending",
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

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use calimero_network_primitives::network_status::{
        AutonatEntry, AutonatTestEntry, AutonatTestKind, DirectUpgradeEntry, DirectUpgradeOutcome,
        NetworkStatusSnapshot, ReachabilityKind, RelayEntry, RelayReservationKind, RendezvousEntry,
        RendezvousRegistrationKind,
    };
    use libp2p::swarm::ConnectionId;
    use libp2p::{Multiaddr, PeerId};

    use super::*;

    fn fixed_ts(secs: u64) -> std::time::SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn addr(s: &str) -> Multiaddr {
        s.parse().unwrap()
    }

    fn empty_snapshot() -> NetworkStatusSnapshot {
        NetworkStatusSnapshot {
            local_peer_id: PeerId::random(),
            listen_addrs: vec![],
            external_addrs: vec![],
            relays: vec![],
            rendezvous: vec![],
            direct_upgrades: vec![],
            autonat: AutonatEntry {
                reachability: ReachabilityKind::Unknown,
                last_test: None,
            },
        }
    }

    #[test]
    fn render_empty_snapshot_has_empty_collections_and_unknown_autonat() {
        let resp = render(empty_snapshot());
        assert!(resp.listen_addrs.is_empty());
        assert!(resp.external_addrs.is_empty());
        assert!(resp.relays.is_empty());
        assert!(resp.rendezvous.is_empty());
        assert!(resp.direct_upgrades.is_empty());
        assert_eq!(resp.autonat.reachability, "unknown");
        assert_eq!(resp.autonat.last_test_addr, None);
        assert_eq!(resp.autonat.last_test_result, None);
        assert_eq!(resp.autonat.last_test_reason, None);
        assert_eq!(resp.autonat.last_test_observed_addr, None);
        assert_eq!(resp.autonat.last_test_at, None);
    }

    #[test]
    fn render_reachable_autonat_populates_observed_addr_and_clears_reason() {
        let mut snap = empty_snapshot();
        snap.autonat = AutonatEntry {
            reachability: ReachabilityKind::Public,
            last_test: Some(AutonatTestEntry {
                tested_addr: addr("/ip4/1.2.3.4/tcp/4001"),
                result: AutonatTestKind::Reachable {
                    addr: addr("/ip4/5.6.7.8/tcp/4001"),
                },
                at: fixed_ts(1_700_000_000),
            }),
        };
        let resp = render(snap);
        assert_eq!(resp.autonat.reachability, "public");
        assert_eq!(resp.autonat.last_test_result.as_deref(), Some("reachable"));
        assert_eq!(
            resp.autonat.last_test_addr.as_deref(),
            Some("/ip4/1.2.3.4/tcp/4001"),
        );
        assert_eq!(
            resp.autonat.last_test_observed_addr.as_deref(),
            Some("/ip4/5.6.7.8/tcp/4001"),
        );
        assert_eq!(resp.autonat.last_test_reason, None);
        assert_eq!(
            resp.autonat.last_test_at.as_deref(),
            Some("2023-11-14T22:13:20Z"),
        );
    }

    #[test]
    fn render_failed_autonat_populates_reason_and_clears_observed_addr() {
        let mut snap = empty_snapshot();
        snap.autonat = AutonatEntry {
            reachability: ReachabilityKind::Private,
            last_test: Some(AutonatTestEntry {
                tested_addr: addr("/ip4/1.2.3.4/tcp/4001"),
                result: AutonatTestKind::Failed {
                    reason: "dial back rejected".to_owned(),
                },
                at: fixed_ts(1_700_000_000),
            }),
        };
        let resp = render(snap);
        assert_eq!(resp.autonat.reachability, "private");
        assert_eq!(resp.autonat.last_test_result.as_deref(), Some("failed"));
        assert_eq!(
            resp.autonat.last_test_reason.as_deref(),
            Some("dial back rejected"),
        );
        assert_eq!(resp.autonat.last_test_observed_addr, None);
    }

    #[test]
    fn render_direct_upgrade_succeeded_uses_display_for_connection_id() {
        let peer = PeerId::random();
        let mut snap = empty_snapshot();
        snap.direct_upgrades = vec![DirectUpgradeEntry {
            peer_id: peer,
            outcome: DirectUpgradeOutcome::Succeeded {
                connection_id: ConnectionId::new_unchecked(42),
            },
            last_attempt: fixed_ts(1_700_000_000),
        }];
        let resp = render(snap);
        assert_eq!(resp.direct_upgrades.len(), 1);
        let entry = &resp.direct_upgrades[0];
        assert_eq!(entry.status, "succeeded");
        assert_eq!(entry.reason, None);
        // Display on ConnectionId writes the bare integer; Debug would write
        // `ConnectionId(42)`. This test pins the choice.
        assert_eq!(entry.connection_id.as_deref(), Some("42"));
    }

    #[test]
    fn render_direct_upgrade_failed_carries_reason_no_connection_id() {
        let peer = PeerId::random();
        let mut snap = empty_snapshot();
        snap.direct_upgrades = vec![DirectUpgradeEntry {
            peer_id: peer,
            outcome: DirectUpgradeOutcome::Failed {
                reason: "hole-punch timeout".to_owned(),
            },
            last_attempt: fixed_ts(1_700_000_000),
        }];
        let resp = render(snap);
        let entry = &resp.direct_upgrades[0];
        assert_eq!(entry.status, "failed");
        assert_eq!(entry.reason.as_deref(), Some("hole-punch timeout"));
        assert_eq!(entry.connection_id, None);
    }

    #[test]
    fn render_relay_and_rendezvous_status_strings_match_kind() {
        let peer = PeerId::random();
        let mut snap = empty_snapshot();
        snap.listen_addrs = vec![addr("/ip4/127.0.0.1/tcp/2428")];
        snap.relays = vec![RelayEntry {
            peer_id: peer,
            reservation_status: RelayReservationKind::Accepted,
            last_state_change: fixed_ts(1_700_000_000),
        }];
        snap.rendezvous = vec![RendezvousEntry {
            peer_id: peer,
            registration_status: RendezvousRegistrationKind::Registered,
            last_state_change: fixed_ts(1_700_000_000),
        }];
        let resp = render(snap);
        assert_eq!(resp.listen_addrs, vec!["/ip4/127.0.0.1/tcp/2428"]);
        assert_eq!(resp.relays[0].reservation_status, "accepted");
        assert_eq!(resp.rendezvous[0].registration_status, "registered");
    }
}
