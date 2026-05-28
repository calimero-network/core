use libp2p::rendezvous::Namespace;

use super::*;

#[test]
fn test_get_preferred_addr() {
    let mut peer_info = PeerInfo::default();
    let tcp_addr_1: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let tcp_addr_2: Multiaddr = "/ip4/127.0.0.1/tcp/4002".parse().unwrap();
    let quic_addr: Multiaddr = "/ip4/127.0.0.1/udp/4001".parse().unwrap();

    assert_eq!(peer_info.get_preferred_addr(), None);

    let _ = peer_info.addrs.insert(tcp_addr_1, 0);
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Tcp(4001)
    );

    let _ = peer_info.addrs.insert(quic_addr, 0);
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Udp(4001)
    );

    let _ = peer_info.addrs.insert(tcp_addr_2, 0);
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Udp(4001)
    );
}

#[test]
fn test_discovery_state_relay_reservation_required() {
    let mut state = DiscoveryState::default();

    let peer1 = PeerId::random();
    let peer2 = PeerId::random();
    let peer3 = PeerId::random();
    let peer4 = PeerId::random();

    state.update_peer_protocols(&peer1, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer2, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer3, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer4, &[HOP_PROTOCOL_NAME]);

    // Initially, no peers have reservations
    assert!(
        state.is_relay_reservation_required(3),
        "Should require reservation when no peers have reservations"
    );

    // Request reservation for one peer
    state.update_relay_reservation_status(&peer1, RelayReservationStatus::Requested);
    assert!(
        state.is_relay_reservation_required(3),
        "Should require reservation when 1 peer has requested reservation"
    );

    // Accept reservation for another peer
    state.update_relay_reservation_status(&peer2, RelayReservationStatus::Accepted);
    assert!(
        state.is_relay_reservation_required(3),
        "Should require reservation when 2 peers have reservations"
    );

    // Request reservation for the third peer
    state.update_relay_reservation_status(&peer3, RelayReservationStatus::Requested);
    assert!(
        !state.is_relay_reservation_required(3),
        "Should not require reservation when 3 peers have reservations"
    );

    // Set the fourth peer to Discovered status (should not affect the count)
    state.update_relay_reservation_status(&peer4, RelayReservationStatus::Discovered);
    assert!(
        !state.is_relay_reservation_required(3),
        "Should not require reservation when 3 peers have reservations and 1 is discovered"
    );

    // Remove a peer with an accepted reservation
    state.remove_peer(&peer2);
    assert!(
        state.is_relay_reservation_required(3),
        "Should require reservation when 1 peer is removed and only 2 have reservations"
    );

    // Change a peer's status to Expired
    state.update_relay_reservation_status(&peer1, RelayReservationStatus::Expired);
    assert!(
        state.is_relay_reservation_required(3),
        "Should require reservation when 1 peer is expired and only 1 has a reservation"
    );

    // Accept reservations for two more peers to reach the limit again
    state.update_relay_reservation_status(&peer1, RelayReservationStatus::Accepted);
    state.update_relay_reservation_status(&peer4, RelayReservationStatus::Requested);
    assert!(
        !state.is_relay_reservation_required(3),
        "Should not require reservation when 3 peers again have reservations"
    );
}

#[test]
fn test_is_rendezvous_discovery_throttled() {
    let mut peer_info = PeerInfo::default();
    assert!(!peer_info.is_rendezvous_discover_throttled(1.0));

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        last_discovery_at: Some(Instant::now().checked_sub(Duration::from_secs(30)).unwrap()),
        ..Default::default()
    });
    assert!(peer_info.is_rendezvous_discover_throttled(1.0));

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        last_discovery_at: Some(Instant::now().checked_sub(Duration::from_secs(61)).unwrap()),
        ..Default::default()
    });
    assert!(!peer_info.is_rendezvous_discover_throttled(1.0));
}

#[test]
fn test_discovery_state_rendezvous_registration_required() {
    let mut state = DiscoveryState::default();

    let peer1 = PeerId::random();
    let peer2 = PeerId::random();
    let peer3 = PeerId::random();
    let peer4 = PeerId::random();

    state.update_peer_protocols(&peer1, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer2, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer3, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer4, &[RENDEZVOUS_PROTOCOL_NAME]);

    // Initially, no peers are registered or requested
    assert!(
        state.is_rendezvous_registration_required(3),
        "Should require registration when no peers are registered/requested"
    );

    // Register one peer
    state.update_rendezvous_registration_status(&peer1, RendezvousRegistrationStatus::Registered);
    assert!(
        state.is_rendezvous_registration_required(3),
        "Should require registration when 1 peer is registered"
    );

    // Request registration for another peer
    state.update_rendezvous_registration_status(&peer2, RendezvousRegistrationStatus::Requested);
    assert!(
        state.is_rendezvous_registration_required(3),
        "Should require registration when 2 peers are registered/requested"
    );

    // Register the third peer
    state.update_rendezvous_registration_status(&peer3, RendezvousRegistrationStatus::Registered);
    assert!(
        !state.is_rendezvous_registration_required(3),
        "Should not require registration when 3 peers are registered/requested"
    );

    // Update the fourth peer to Discovered status (should not affect the count)
    state.update_rendezvous_registration_status(&peer4, RendezvousRegistrationStatus::Discovered);
    assert!(
        !state.is_rendezvous_registration_required(3),
        "Should not require registration when 3 peers are registered/requested and 1 is discovered"
    );

    // Remove a registered peer
    state.remove_peer(&peer1);
    assert!(
        state.is_rendezvous_registration_required(3),
        "Should require registration when 1 peer is removed and only 2 are registered/requested"
    );

    // Change a peer's status to Expired
    state.update_rendezvous_registration_status(&peer2, RendezvousRegistrationStatus::Expired);
    assert!(
        state.is_rendezvous_registration_required(3),
        "Should require registration when 1 peer is expired and only 1 is registered"
    );

    // Register two more peers to reach the limit again
    state.update_rendezvous_registration_status(&peer2, RendezvousRegistrationStatus::Registered);
    state.update_rendezvous_registration_status(&peer4, RendezvousRegistrationStatus::Requested);
    assert!(
        !state.is_rendezvous_registration_required(3),
        "Should not require registration when 3 peers are again registered/requested"
    );
}

#[test]
fn test_state_mutations() {
    let mut state = DiscoveryState::default();
    let peer_id = PeerId::random();
    let tcp_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let quic_addr: Multiaddr = "/ip4/127.0.0.1/udp/4001".parse().unwrap();
    let protocols = vec![HOP_PROTOCOL_NAME, RENDEZVOUS_PROTOCOL_NAME];
    let mdns_discovery = PeerDiscoveryMechanism::Mdns;
    let rendezvous_discovery = PeerDiscoveryMechanism::Rendezvous;
    let cookie = Cookie::for_namespace(Namespace::from_static("test"));
    let relay_status = RelayReservationStatus::Accepted;
    let rendezvous_status = RendezvousRegistrationStatus::Registered;

    state.update_peer_protocols(&peer_id, &protocols);
    assert!(state.is_peer_relay(&peer_id));
    assert!(state.is_peer_rendezvous(&peer_id));
    assert_eq!(
        state.peers[&peer_id]
            .rendezvous
            .as_ref()
            .unwrap()
            .registration_status(),
        RendezvousRegistrationStatus::Discovered
    );
    assert_eq!(
        state.peers[&peer_id]
            .relay
            .as_ref()
            .unwrap()
            .reservation_status(),
        RelayReservationStatus::Discovered
    );

    state.add_peer_addr(peer_id, &quic_addr);
    state.add_peer_addr(peer_id, &tcp_addr);
    assert_eq!(state.peers.len(), 1);
    assert_eq!(state.peers[&peer_id].addrs.len(), 2);
    assert!(state.peers[&peer_id].addrs.contains_key(&tcp_addr));
    assert!(state.peers[&peer_id].addrs.contains_key(&quic_addr));

    state.add_peer_discovery_mechanism(&peer_id, mdns_discovery);
    state.add_peer_discovery_mechanism(&peer_id, rendezvous_discovery);
    assert_eq!(state.peers[&peer_id].discoveries.len(), 2);
    assert!(state.is_peer_discovered_via(&peer_id, mdns_discovery));
    assert!(state.is_peer_discovered_via(&peer_id, rendezvous_discovery));

    state.update_rendezvous_cookie(&peer_id, &cookie);
    state.update_rendezvous_registration_status(&peer_id, rendezvous_status);
    state.update_relay_reservation_status(&peer_id, relay_status);
    assert_eq!(
        state.peers[&peer_id].rendezvous.as_ref().unwrap().cookie(),
        Some(&cookie)
    );
    assert!(state.peers[&peer_id]
        .rendezvous
        .as_ref()
        .unwrap()
        .last_discovery_at()
        .is_some());
    assert_eq!(
        state.peers[&peer_id]
            .rendezvous
            .as_ref()
            .unwrap()
            .registration_status(),
        rendezvous_status
    );
    assert_eq!(
        state.peers[&peer_id]
            .relay
            .as_ref()
            .unwrap()
            .reservation_status(),
        relay_status
    );

    state.remove_peer(&peer_id);
    assert_eq!(state.peers.len(), 0);
    assert_eq!(state.relay_index.len(), 0);
    assert_eq!(state.rendezvous_index.len(), 0);
}

#[test]
fn test_on_relay_reservation_lost_marks_expired_and_queues_recovery() {
    let mut state = DiscoveryState::default();
    let relay_peer = PeerId::random();

    state.update_peer_protocols(&relay_peer, &[HOP_PROTOCOL_NAME]);
    state.update_relay_reservation_status(&relay_peer, RelayReservationStatus::Accepted);

    let actions = state.on_relay_reservation_lost(&relay_peer);

    assert_eq!(
        state.peers[&relay_peer]
            .relay
            .as_ref()
            .unwrap()
            .reservation_status(),
        RelayReservationStatus::Expired,
        "lost reservation should transition to Expired"
    );
    assert_eq!(
        actions.relay_reservations,
        vec![relay_peer],
        "recovery action should queue the relay peer for re-request"
    );
    assert!(
        actions.has_actions(),
        "loss of an Accepted reservation must produce a non-empty action set"
    );
}

#[test]
fn test_on_relay_reservation_lost_from_requested_does_not_queue() {
    let mut state = DiscoveryState::default();
    let relay_peer = PeerId::random();

    state.update_peer_protocols(&relay_peer, &[HOP_PROTOCOL_NAME]);
    state.update_relay_reservation_status(&relay_peer, RelayReservationStatus::Requested);

    let actions = state.on_relay_reservation_lost(&relay_peer);

    // From Requested, status flips to Expired but no recovery is queued.
    // Requested means either a pending request that just failed (queuing
    // would loop on a deliberate denial) or a stale event for a prior loss
    // whose recovery is already in flight (queuing would duplicate it).
    assert!(actions.relay_reservations.is_empty());
    assert_eq!(
        state.peers[&relay_peer]
            .relay
            .as_ref()
            .unwrap()
            .reservation_status(),
        RelayReservationStatus::Expired,
        "status flips to Expired even when no recovery is queued"
    );
}

#[test]
fn test_on_relay_reservation_lost_is_idempotent_under_event_burst() {
    let mut state = DiscoveryState::default();
    let relay_peer = PeerId::random();

    state.update_peer_protocols(&relay_peer, &[HOP_PROTOCOL_NAME]);
    state.update_relay_reservation_status(&relay_peer, RelayReservationStatus::Accepted);

    // Burst simulating one disconnect: ConnectionClosed -> ListenerClosed ->
    // ExternalAddrExpired arrive in quick succession. Between the first
    // event and the next, NetworkManager's execute_reachability_actions
    // calls create_relay_reservation, which flips Expired -> Requested for
    // the in-flight new reservation.
    let after_connection_closed = state.on_relay_reservation_lost(&relay_peer);
    assert_eq!(
        after_connection_closed.relay_reservations,
        vec![relay_peer],
        "first event of the burst queues exactly one recovery"
    );

    // Simulate create_relay_reservation completing successfully and setting
    // status to Requested on its new libp2p listener.
    state.update_relay_reservation_status(&relay_peer, RelayReservationStatus::Requested);

    let after_listener_closed = state.on_relay_reservation_lost(&relay_peer);
    assert!(
        after_listener_closed.relay_reservations.is_empty(),
        "second event in the burst (ListenerClosed for the dead listener) \
         must not queue another listen_on; the recovery is already in flight"
    );

    // After the second event, status is back to Expired (the in-flight
    // libp2p listener keeps running until ExternalAddrConfirmed lands).
    let after_external_addr_expired = state.on_relay_reservation_lost(&relay_peer);
    assert!(
        after_external_addr_expired.relay_reservations.is_empty(),
        "third event in the burst is also a no-op"
    );
}

#[test]
fn test_on_relay_reservation_lost_for_discovered_peer_is_noop() {
    let mut state = DiscoveryState::default();
    let relay_peer = PeerId::random();

    state.update_peer_protocols(&relay_peer, &[HOP_PROTOCOL_NAME]);
    // Discovered means we know the peer speaks the hop protocol but never
    // asked it for a reservation. Losing "nothing" should be a no-op and
    // leave status untouched — there is nothing to mark Expired.

    let actions = state.on_relay_reservation_lost(&relay_peer);

    assert!(actions.relay_reservations.is_empty());
    assert_eq!(
        state.peers[&relay_peer]
            .relay
            .as_ref()
            .unwrap()
            .reservation_status(),
        RelayReservationStatus::Discovered,
        "status stays Discovered; nothing to mark Expired"
    );
}

#[test]
fn test_on_relay_reservation_lost_for_non_relay_peer_is_noop() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();

    // Peer is not in relay_index — e.g. a rendezvous-only peer.
    state.update_peer_protocols(&peer, &[RENDEZVOUS_PROTOCOL_NAME]);

    let actions = state.on_relay_reservation_lost(&peer);

    assert!(actions.relay_reservations.is_empty());
    assert!(!actions.has_actions());
}

#[test]
fn test_on_relay_reservation_lost_multiple_relays_queue_independently() {
    let mut state = DiscoveryState::default();
    let relay_a = PeerId::random();
    let relay_b = PeerId::random();

    state.update_peer_protocols(&relay_a, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&relay_b, &[HOP_PROTOCOL_NAME]);
    state.update_relay_reservation_status(&relay_a, RelayReservationStatus::Accepted);
    state.update_relay_reservation_status(&relay_b, RelayReservationStatus::Accepted);

    let actions_a = state.on_relay_reservation_lost(&relay_a);
    assert_eq!(actions_a.relay_reservations, vec![relay_a]);

    // Losing the second relay should still produce a recovery action, even
    // though the first is already Expired. Each relay is tracked independently.
    let actions_b = state.on_relay_reservation_lost(&relay_b);
    assert_eq!(actions_b.relay_reservations, vec![relay_b]);
}

#[test]
fn test_add_peer_addr_initialises_failure_counter_to_zero() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();

    state.add_peer_addr(peer, &addr);
    assert_eq!(state.peers[&peer].addrs[&addr], 0);
}

#[test]
fn test_add_peer_addr_resets_failure_counter_when_address_already_present() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();

    state.add_peer_addr(peer, &addr);
    let _ = state.record_dial_failure(&peer, &addr);
    let _ = state.record_dial_failure(&peer, &addr);
    assert_eq!(state.peers[&peer].addrs[&addr], 2);

    // A re-addition (successful identify push, fresh dial, etc.) treats
    // the address as healthy again and resets the counter.
    state.add_peer_addr(peer, &addr);
    assert_eq!(state.peers[&peer].addrs[&addr], 0);
}

#[test]
fn test_record_dial_failure_evicts_at_threshold() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();

    state.add_peer_addr(peer, &addr);
    for _ in 0..(DIAL_FAILURE_EVICTION_THRESHOLD - 1) {
        assert!(
            !state.record_dial_failure(&peer, &addr),
            "address must not evict before reaching the threshold"
        );
    }
    assert!(state.peers[&peer].addrs.contains_key(&addr));

    // The threshold-th failure evicts.
    let evicted = state.record_dial_failure(&peer, &addr);
    assert!(evicted, "reaching the threshold must evict");
    assert!(!state.peers[&peer].addrs.contains_key(&addr));
}

#[test]
fn test_record_dial_failure_is_noop_for_unknown_peer_or_address() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let other_addr: Multiaddr = "/ip4/127.0.0.1/tcp/4002".parse().unwrap();

    // Unknown peer: no-op.
    assert!(!state.record_dial_failure(&peer, &addr));
    assert!(!state.peers.contains_key(&peer));

    // Known peer, unknown address: no-op (don't speculatively insert an
    // address we never planned to keep).
    state.add_peer_addr(peer, &addr);
    assert!(!state.record_dial_failure(&peer, &other_addr));
    assert!(!state.peers[&peer].addrs.contains_key(&other_addr));
}

#[test]
fn test_dial_success_after_failures_resets_counter() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    let addr: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();

    state.add_peer_addr(peer, &addr);
    let _ = state.record_dial_failure(&peer, &addr);
    let _ = state.record_dial_failure(&peer, &addr);
    assert_eq!(state.peers[&peer].addrs[&addr], 2);

    // ConnectionEstablished re-records the address, resetting the counter.
    // The address now needs another full threshold of consecutive
    // failures before eviction.
    state.add_peer_addr(peer, &addr);
    assert_eq!(state.peers[&peer].addrs[&addr], 0);

    for _ in 0..(DIAL_FAILURE_EVICTION_THRESHOLD - 1) {
        assert!(!state.record_dial_failure(&peer, &addr));
    }
    assert!(state.peers[&peer].addrs.contains_key(&addr));
}

#[test]
fn test_independent_addresses_track_failures_separately() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    let addr_a: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let addr_b: Multiaddr = "/ip4/127.0.0.1/tcp/4002".parse().unwrap();

    state.add_peer_addr(peer, &addr_a);
    state.add_peer_addr(peer, &addr_b);

    // Fail addr_a to the threshold; addr_b is untouched.
    for _ in 0..DIAL_FAILURE_EVICTION_THRESHOLD {
        let _ = state.record_dial_failure(&peer, &addr_a);
    }

    assert!(!state.peers[&peer].addrs.contains_key(&addr_a));
    assert!(state.peers[&peer].addrs.contains_key(&addr_b));
    assert_eq!(state.peers[&peer].addrs[&addr_b], 0);
}

#[test]
fn test_take_relay_listener_returns_recorded_peer_and_removes_entry() {
    let mut state = DiscoveryState::default();
    let relay = PeerId::random();
    let listener = ListenerId::next();

    state.record_relay_listener(listener, relay);
    assert_eq!(state.take_relay_listener(&listener), Some(relay));

    // Second take returns None — the entry was removed.
    assert_eq!(state.take_relay_listener(&listener), None);
}

#[test]
fn test_record_relay_listener_overwrites_existing_entry() {
    let mut state = DiscoveryState::default();
    let relay_a = PeerId::random();
    let relay_b = PeerId::random();
    let listener = ListenerId::next();

    state.record_relay_listener(listener, relay_a);
    // Same listener id reused (unusual but the API must be consistent):
    // the latest registration wins.
    state.record_relay_listener(listener, relay_b);

    assert_eq!(state.take_relay_listener(&listener), Some(relay_b));
}

#[test]
fn test_take_relay_listener_is_noop_for_unknown_id() {
    let mut state = DiscoveryState::default();
    let unknown = ListenerId::next();

    // Taking an id we never recorded must not panic and must return None.
    assert_eq!(state.take_relay_listener(&unknown), None);
}

#[test]
fn test_multiple_listeners_map_to_distinct_relays() {
    let mut state = DiscoveryState::default();
    let relay_a = PeerId::random();
    let relay_b = PeerId::random();
    let listener_a = ListenerId::next();
    let listener_b = ListenerId::next();

    state.record_relay_listener(listener_a, relay_a);
    state.record_relay_listener(listener_b, relay_b);

    // Taking one leaves the other intact.
    assert_eq!(state.take_relay_listener(&listener_a), Some(relay_a));
    assert_eq!(state.take_relay_listener(&listener_a), None);
    assert_eq!(state.take_relay_listener(&listener_b), Some(relay_b));
}

/// Spin until the monotonic clock has strictly advanced past `prior`,
/// bounded by a generous 100ms ceiling. Replaces `thread::sleep(2ms)`,
/// which was flaky on slow CI where the kernel scheduler doesn't wake
/// within the requested window and on Windows where `Instant`
/// resolution is coarser than 1ms.
fn wait_past(prior: std::time::Instant) {
    let deadline = prior + std::time::Duration::from_millis(100);
    while std::time::Instant::now() <= prior {
        if std::time::Instant::now() > deadline {
            panic!("clock failed to advance within 100ms — broken Instant");
        }
        std::hint::spin_loop();
    }
}

#[test]
fn record_dcutr_outcome_replaces_prior_observation() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();

    // First observation: a failure. ensure_peer side-effect should create
    // a PeerInfo entry even though we never identified the peer.
    state.record_dcutr_outcome(
        peer,
        DcutrUpgradeStatus::Failed {
            reason: "timeout".to_owned(),
        },
    );
    let info = state.get_peer_info(&peer).expect("peer created");
    let first = info.dcutr().expect("dcutr observation recorded");
    let first_at = first.at();
    assert!(matches!(first.status(), DcutrUpgradeStatus::Failed { .. }));

    // A subsequent success must overwrite the failure and bump the timestamp.
    wait_past(first_at);
    state.record_dcutr_outcome(
        peer,
        DcutrUpgradeStatus::Succeeded {
            connection_id: libp2p::swarm::ConnectionId::new_unchecked(7),
        },
    );
    let second = state
        .get_peer_info(&peer)
        .and_then(|i| i.dcutr())
        .expect("still recorded");
    assert!(matches!(
        second.status(),
        DcutrUpgradeStatus::Succeeded { .. }
    ));
    assert!(
        second.at() > first_at,
        "second observation must postdate the first",
    );
}

#[test]
fn record_autonat_test_keeps_only_the_freshest_probe() {
    let mut state = DiscoveryState::default();
    let addr_one: Multiaddr = "/ip4/1.2.3.4/tcp/1234".parse().unwrap();
    let addr_two: Multiaddr = "/ip4/5.6.7.8/tcp/5678".parse().unwrap();

    state.record_autonat_test(
        addr_one.clone(),
        AutonatTestResult::Failed {
            reason: "no servers".to_owned(),
        },
    );
    let first_at = state.last_autonat_test().expect("recorded").at;

    wait_past(first_at);
    state.record_autonat_test(
        addr_two.clone(),
        AutonatTestResult::Reachable {
            addr: addr_two.clone(),
        },
    );

    let probe = state.last_autonat_test().expect("still recorded");
    assert_eq!(probe.tested_addr, addr_two);
    assert!(matches!(probe.result, AutonatTestResult::Reachable { .. }));
    assert!(probe.at > first_at, "second probe must postdate first");
}

#[test]
fn relay_reservation_status_change_bumps_last_state_change() {
    let mut state = DiscoveryState::default();
    let relay = PeerId::random();
    state.update_peer_protocols(&relay, &[HOP_PROTOCOL_NAME]);

    let baseline = state
        .get_peer_info(&relay)
        .and_then(|i| i.relay())
        .map(|r| r.last_state_change())
        .expect("relay info initialized");

    wait_past(baseline);
    state.update_relay_reservation_status(&relay, RelayReservationStatus::Accepted);

    let after = state
        .get_peer_info(&relay)
        .and_then(|i| i.relay())
        .map(|r| r.last_state_change())
        .expect("still tracked");
    assert!(
        after > baseline,
        "last_state_change must advance on status update",
    );
}
