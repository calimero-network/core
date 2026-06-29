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
fn test_pending_rendezvous_registration_does_not_occupy_slot() {
    let mut state = DiscoveryState::default();

    let peer1 = PeerId::random();
    let peer2 = PeerId::random();

    state.update_peer_protocols(&peer1, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&peer2, &[RENDEZVOUS_PROTOCOL_NAME]);

    // A peer that tried to register but had no external address yet is marked
    // Pending. Like Discovered/Expired, Pending is NOT a real registration, so
    // it must not satisfy the fan-out gate — otherwise the node would believe
    // it is registered and never re-attempt once an external address arrives.
    state.update_rendezvous_registration_status(&peer1, RendezvousRegistrationStatus::Pending);
    assert!(
        state.is_rendezvous_registration_required(1),
        "Pending must not occupy a registration slot (registration still required)"
    );

    // Once the registration actually goes out (Requested), the slot is taken.
    state.update_rendezvous_registration_status(&peer1, RendezvousRegistrationStatus::Requested);
    assert!(
        !state.is_rendezvous_registration_required(1),
        "Requested occupies the slot (no further registration required at limit 1)"
    );

    // Sanity: a second Pending peer still doesn't count toward the limit.
    state.update_rendezvous_registration_status(&peer2, RendezvousRegistrationStatus::Pending);
    assert!(
        !state.is_rendezvous_registration_required(1),
        "Pending peer2 adds no slot; the single Requested peer already meets limit 1"
    );
}

#[test]
fn test_pending_to_expired_transition_keeps_slot_free() {
    let mut state = DiscoveryState::default();

    let peer = PeerId::random();
    state.update_peer_protocols(&peer, &[RENDEZVOUS_PROTOCOL_NAME]);

    // Pending holds no slot, so registration is still required.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Pending);
    assert!(
        state.is_rendezvous_registration_required(1),
        "Pending occupies no slot"
    );

    // Transitioning Pending -> Expired (e.g. an unregister/expiry path
    // touching a never-registered peer) must not invent a slot: neither
    // status counts, so registration stays required and the count is
    // unchanged.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Expired);
    assert!(
        state.is_rendezvous_registration_required(1),
        "Expired (like Pending) occupies no slot"
    );
}

fn rendezvous_status(state: &DiscoveryState, peer: &PeerId) -> RendezvousRegistrationStatus {
    state.peers[peer]
        .rendezvous
        .as_ref()
        .unwrap()
        .registration_status()
}

#[test]
fn test_mark_rendezvous_pending_if_idle_guards_slot_holders() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();
    state.update_peer_protocols(&peer, &[RENDEZVOUS_PROTOCOL_NAME]);

    // Idle statuses (Discovered / Expired / already Pending) move to Pending.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Discovered);
    assert!(state.mark_rendezvous_pending_if_idle(&peer));
    assert_eq!(
        rendezvous_status(&state, &peer),
        RendezvousRegistrationStatus::Pending
    );

    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Expired);
    assert!(state.mark_rendezvous_pending_if_idle(&peer));
    assert_eq!(
        rendezvous_status(&state, &peer),
        RendezvousRegistrationStatus::Pending
    );

    // Requested must NOT be clobbered: a register is in flight, and the
    // Registered event handler drops the confirmation unless status is
    // still Requested. The call is a no-op and reports no change.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Requested);
    assert!(!state.mark_rendezvous_pending_if_idle(&peer));
    assert_eq!(
        rendezvous_status(&state, &peer),
        RendezvousRegistrationStatus::Requested
    );

    // Registered must NOT be clobbered: it holds a live server record and a
    // fan-out slot that must keep counting.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Registered);
    assert!(!state.mark_rendezvous_pending_if_idle(&peer));
    assert_eq!(
        rendezvous_status(&state, &peer),
        RendezvousRegistrationStatus::Registered
    );
}

#[test]
fn test_find_new_rendezvous_peer_nominates_pending() {
    let mut state = DiscoveryState::default();

    let peer = PeerId::random();
    state.update_peer_protocols(&peer, &[RENDEZVOUS_PROTOCOL_NAME]);

    // A lone Pending peer (tried, blocked on a missing external address)
    // must still be nominatable. The Expired-event handler relies on this
    // to re-drive registration once a slot frees; skipping Pending would
    // strand the peer until the next ExternalAddrConfirmed.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Pending);
    assert_eq!(
        state.find_new_rendezvous_peer(),
        Some(peer),
        "a Pending peer must be nominated, like Discovered"
    );

    // Once registration is in flight (Requested) or live (Registered) the
    // peer occupies a slot and is no longer a nomination target.
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Requested);
    assert_eq!(
        state.find_new_rendezvous_peer(),
        None,
        "a Requested peer is not a nomination target"
    );
    state.update_rendezvous_registration_status(&peer, RendezvousRegistrationStatus::Registered);
    assert_eq!(state.find_new_rendezvous_peer(), None);
}

#[test]
fn test_find_new_rendezvous_peer_prefers_pending_over_expired() {
    let mut state = DiscoveryState::default();

    // `get_rendezvous_peer_ids` iterates the BTreeSet in PeerId order, so
    // assign roles deterministically: the Expired peer sorts *first* and
    // is therefore encountered (and recorded as the fallback candidate)
    // before the Pending peer. This proves the eager Pending peer wins via
    // early return even when an Expired candidate was already found — the
    // preference is not an artifact of iteration order.
    let mut ids = [PeerId::random(), PeerId::random()];
    ids.sort();
    let expired_peer = ids[0];
    let pending_peer = ids[1];
    // Pin the ordering assumption loudly: if get_rendezvous_peer_ids ever
    // stops iterating in ascending PeerId order, this fails here rather
    // than silently no longer exercising the early-return-over-candidate
    // path.
    assert!(
        expired_peer < pending_peer,
        "expired_peer must sort before pending_peer for this test to mean anything"
    );
    state.update_peer_protocols(&expired_peer, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&pending_peer, &[RENDEZVOUS_PROTOCOL_NAME]);

    // Expired = was registered, lapsed (fallback). Pending = registerable
    // target holding no slot (eager).
    state.update_rendezvous_registration_status(
        &expired_peer,
        RendezvousRegistrationStatus::Expired,
    );
    state.update_rendezvous_registration_status(
        &pending_peer,
        RendezvousRegistrationStatus::Pending,
    );
    assert_eq!(
        state.find_new_rendezvous_peer(),
        Some(pending_peer),
        "Pending (eager) wins over an already-found Expired (fallback) candidate"
    );

    // With only an Expired peer left, it is the fallback nomination.
    state.update_rendezvous_registration_status(
        &pending_peer,
        RendezvousRegistrationStatus::Registered,
    );
    assert_eq!(
        state.find_new_rendezvous_peer(),
        Some(expired_peer),
        "Expired peer is nominated when no eager target exists"
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
fn test_add_peer_addr_caps_addresses_per_peer() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();

    // Advertise far more distinct addresses than the cap allows.
    for port in 0..(MAX_ADDRS_PER_PEER as u16 + 5) {
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4001 + port)
            .parse()
            .unwrap();
        state.add_peer_addr(peer, &addr);
    }

    assert_eq!(
        state.peers[&peer].addrs.len(),
        MAX_ADDRS_PER_PEER,
        "per-peer address book must be capped"
    );
}

#[test]
fn test_add_peer_addr_evicts_worst_address_when_capped() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();

    // Fill the book to capacity.
    let mut addrs = Vec::new();
    for port in 0..(MAX_ADDRS_PER_PEER as u16) {
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4001 + port)
            .parse()
            .unwrap();
        state.add_peer_addr(peer, &addr);
        addrs.push(addr);
    }

    // Give the first address some (sub-threshold) dial failures so it is
    // the worst — but not enough to be evicted by the failure path.
    let worst = &addrs[0];
    for _ in 0..(DIAL_FAILURE_EVICTION_THRESHOLD - 1) {
        let _ = state.record_dial_failure(&peer, worst);
    }

    // Adding a fresh address must evict the worst one, not a healthy one.
    let fresh: Multiaddr = "/ip4/127.0.0.1/tcp/5999".parse().unwrap();
    state.add_peer_addr(peer, &fresh);

    assert_eq!(state.peers[&peer].addrs.len(), MAX_ADDRS_PER_PEER);
    assert!(
        !state.peers[&peer].addrs.contains_key(worst),
        "the address with the most failures must be evicted first"
    );
    assert!(
        state.peers[&peer].addrs.contains_key(&fresh),
        "the freshly added address must be retained"
    );
}

#[test]
fn test_add_peer_addr_evicts_oldest_when_all_healthy() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();

    // Fill the book to capacity with all-healthy (zero-failure) addresses.
    let mut addrs = Vec::new();
    for port in 0..(MAX_ADDRS_PER_PEER as u16) {
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4001 + port)
            .parse()
            .unwrap();
        state.add_peer_addr(peer, &addr);
        addrs.push(addr);
    }

    // With no failures to distinguish them, eviction is deterministic:
    // the oldest (first-inserted) address is dropped and the newest is
    // kept — mirroring the peer-cache's newest-first policy so an IP
    // change is captured rather than pinned out.
    let fresh: Multiaddr = "/ip4/127.0.0.1/tcp/5999".parse().unwrap();
    state.add_peer_addr(peer, &fresh);

    assert_eq!(state.peers[&peer].addrs.len(), MAX_ADDRS_PER_PEER);
    assert!(
        !state.peers[&peer].addrs.contains_key(&addrs[0]),
        "the oldest address must be evicted on an all-healthy tie"
    );
    assert!(state.peers[&peer].addrs.contains_key(&fresh));
    // Every later (younger) healthy address survives.
    for addr in &addrs[1..] {
        assert!(state.peers[&peer].addrs.contains_key(addr));
    }
}

#[test]
fn test_add_peer_addr_refresh_of_existing_does_not_evict() {
    let mut state = DiscoveryState::default();
    let peer = PeerId::random();

    // Fill the book to capacity.
    let mut addrs = Vec::new();
    for port in 0..(MAX_ADDRS_PER_PEER as u16) {
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", 4001 + port)
            .parse()
            .unwrap();
        state.add_peer_addr(peer, &addr);
        addrs.push(addr);
    }

    // Re-adding an address that is already present is a refresh: it resets
    // the counter without evicting any sibling, and the book stays full
    // with the same membership.
    let _ = state.record_dial_failure(&peer, &addrs[0]);
    state.add_peer_addr(peer, &addrs[0]);

    assert_eq!(state.peers[&peer].addrs.len(), MAX_ADDRS_PER_PEER);
    for addr in &addrs {
        assert!(state.peers[&peer].addrs.contains_key(addr));
    }
    assert_eq!(state.peers[&peer].addrs[&addrs[0]], 0);
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

// ---------------------------------------------------------------------------
// on_regular_peer_disconnected — the #2469 recovery path
//
// These tests pin the contract that the `SwarmEvent::ConnectionClosed` branch
// in `crates/network/src/handlers/stream/swarm.rs` relies on:
//
//   - When a regular peer's connection drops, the action set must contain
//     every known rendezvous peer in `rendezvous_discover_force` so the
//     caller can fire an immediate (throttle-bypassed) discover request.
//   - The throttled `rendezvous_discover` field must stay empty — otherwise
//     the periodic-tick path would silently swallow these events when the
//     `discovery_rpm` floor is still in effect (the bug we're fixing).
//   - The method must be a no-op when no rendezvous peers are known (mdns-
//     only / local-loopback deployments); we don't want bogus actions to
//     trip the action dispatcher's `has_actions()` early-return.
// ---------------------------------------------------------------------------

#[test]
fn test_on_regular_peer_disconnected_queues_force_discover_for_known_rendezvous() {
    let mut state = DiscoveryState::default();
    let rendezvous = PeerId::random();
    state.update_peer_protocols(&rendezvous, &[RENDEZVOUS_PROTOCOL_NAME]);

    let actions = state.on_regular_peer_disconnected();

    assert_eq!(
        actions.rendezvous_discover_force,
        vec![rendezvous],
        "the lone known rendezvous peer should be queued for force-discovery"
    );
    assert!(
        actions.rendezvous_discover.is_empty(),
        "the throttled path must NOT be populated; that path is for periodic ticks \
         and would be no-op'd by the discovery_rpm floor right after a disconnect"
    );
    assert!(
        actions.has_actions(),
        "force-discover queue is a real action; has_actions must reflect that or \
         the executor will short-circuit before dispatching it"
    );
}

#[test]
fn test_on_regular_peer_disconnected_returns_all_rendezvous_peers() {
    let mut state = DiscoveryState::default();
    let r1 = PeerId::random();
    let r2 = PeerId::random();
    let r3 = PeerId::random();
    state.update_peer_protocols(&r1, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&r2, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&r3, &[RENDEZVOUS_PROTOCOL_NAME]);

    let actions = state.on_regular_peer_disconnected();

    // Order is not contractual (rendezvous peers live in a set), but every
    // one of them must be queued so we re-query each independently. In
    // production we typically run one rendezvous peer (the boot-node), but
    // multi-rendezvous deployments do exist and we want symmetric behavior.
    let mut got: Vec<_> = actions.rendezvous_discover_force.clone();
    got.sort();
    let mut want = vec![r1, r2, r3];
    want.sort();
    assert_eq!(got, want, "every known rendezvous peer must be queued");
}

#[test]
fn test_on_regular_peer_disconnected_is_noop_without_rendezvous_peers() {
    // mdns-only / local-loopback scenarios have no rendezvous server to
    // re-query. The method must surface this as a no-op so the dispatcher's
    // `if !actions.has_actions() { return; }` short-circuit fires.
    let state = DiscoveryState::default();

    let actions = state.on_regular_peer_disconnected();

    assert!(actions.rendezvous_discover_force.is_empty());
    assert!(actions.rendezvous_discover.is_empty());
    assert!(
        !actions.has_actions(),
        "no rendezvous peers known → no actions; otherwise the dispatcher \
         would spin on every disconnect of every mdns peer"
    );
}

#[test]
fn test_on_regular_peer_disconnected_ignores_non_rendezvous_peers() {
    // A relay-only peer and a HOP+RENDEZVOUS dual-role peer share the
    // rendezvous slot. The discriminator is `is_peer_rendezvous` which is
    // driven by protocol announcement. The mere presence of a relay peer
    // must NOT cause it to appear in the force-discover queue.
    let mut state = DiscoveryState::default();
    let relay = PeerId::random();
    let rendezvous = PeerId::random();
    state.update_peer_protocols(&relay, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&rendezvous, &[RENDEZVOUS_PROTOCOL_NAME]);

    let actions = state.on_regular_peer_disconnected();

    assert_eq!(
        actions.rendezvous_discover_force,
        vec![rendezvous],
        "only rendezvous-protocol peers belong in the discover queue"
    );
    assert!(
        !actions.rendezvous_discover_force.contains(&relay),
        "relay-only peers must NOT be force-discovered (they don't serve \
         rendezvous-discover requests)"
    );
}

#[test]
fn test_on_regular_peer_disconnected_includes_dual_role_peers() {
    // A peer announcing both HOP and RENDEZVOUS protocols is the boot-node
    // shape in the e2e workflows (single binary serving relay + rendezvous).
    // Such a peer must be queued for discover (it serves the rendezvous
    // protocol) regardless of the HOP role.
    let mut state = DiscoveryState::default();
    let boot_node = PeerId::random();
    state.update_peer_protocols(&boot_node, &[HOP_PROTOCOL_NAME, RENDEZVOUS_PROTOCOL_NAME]);

    let actions = state.on_regular_peer_disconnected();

    assert_eq!(
        actions.rendezvous_discover_force,
        vec![boot_node],
        "dual-role boot-node peers must be queued via their rendezvous role"
    );
}

// ---------------------------------------------------------------------------
// has_regular_connected_peer — post-restart force-rediscovery gate
//
// The rendezvous tick uses this to decide whether to bypass the
// `discovery_rpm` throttle. The contract:
//
//   - A connection set containing only infrastructure peers (relay and/or
//     rendezvous) counts as "peerless" → returns false → tick bypasses the
//     throttle and re-discovers every interval. This is the post-restart
//     shape (no `ConnectionClosed` ever fires, so #2469 can't help).
//   - Any single regular (non-relay, non-rendezvous) connection flips it to
//     true → throttle re-engages so we don't hammer rendezvous once the
//     overlay is healthy.
//   - A dual-role HOP+RENDEZVOUS boot-node is infrastructure, not a regular
//     peer — a node connected only to the boot-node is still partitioned.
// ---------------------------------------------------------------------------

#[test]
fn test_has_regular_connected_peer_empty_is_false() {
    let state = DiscoveryState::default();
    assert!(
        !state.has_regular_connected_peer(std::iter::empty()),
        "no connections at all → not connected to any regular peer"
    );
}

#[test]
fn test_has_regular_connected_peer_only_infra_is_false() {
    // Connected solely to a relay and a rendezvous server — the exact
    // post-restart NAT shape. Must read as peerless so the tick force-
    // rediscovers instead of waiting the throttle floor.
    let mut state = DiscoveryState::default();
    let relay = PeerId::random();
    let rendezvous = PeerId::random();
    state.update_peer_protocols(&relay, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&rendezvous, &[RENDEZVOUS_PROTOCOL_NAME]);

    let connected = [relay, rendezvous];
    assert!(
        !state.has_regular_connected_peer(connected.iter()),
        "relay + rendezvous only → still partitioned from the app overlay"
    );
}

#[test]
fn test_has_regular_connected_peer_with_regular_is_true() {
    let mut state = DiscoveryState::default();
    let relay = PeerId::random();
    let regular = PeerId::random();
    state.update_peer_protocols(&relay, &[HOP_PROTOCOL_NAME]);
    // `regular` is never classified as relay/rendezvous, so it stays a
    // regular peer (the discriminator is index membership, not presence
    // in the peers map).

    let connected = [relay, regular];
    assert!(
        state.has_regular_connected_peer(connected.iter()),
        "one regular connection alongside infra → overlay reachable"
    );
}

#[test]
fn test_has_regular_connected_peer_dual_role_bootnode_is_infra() {
    // A single HOP+RENDEZVOUS boot-node is infrastructure. A node whose
    // only connection is the boot-node is still effectively peerless and
    // must keep force-rediscovering.
    let mut state = DiscoveryState::default();
    let boot_node = PeerId::random();
    state.update_peer_protocols(&boot_node, &[HOP_PROTOCOL_NAME, RENDEZVOUS_PROTOCOL_NAME]);

    let connected = [boot_node];
    assert!(
        !state.has_regular_connected_peer(connected.iter()),
        "dual-role boot-node is infra, not a regular peer"
    );
}

// ---------------------------------------------------------------------------
// rendezvous_key_for_topic — per-overlay rendezvous key derivation
//
// Each subscribed gossipsub topic maps to a distinct rendezvous key so
// `discover` returns only co-members of that exact overlay. The mapping
// must be deterministic so a registering member and a discovering peer
// (which holds the same id) compute the identical key.
// ---------------------------------------------------------------------------

#[test]
fn test_rendezvous_key_for_namespace_topic() {
    let hex = "8a6157eacc0e68d1786a585891866794d6fc5c11a199dbcb81ed33f5759a37a1";
    let key = rendezvous_key_for_topic(&format!("ns/{hex}")).expect("namespace key");
    assert_eq!(key, *format!("/calimero/ns/{hex}").as_str());
}

#[test]
fn test_rendezvous_key_for_group_topic() {
    let hex = "20d12aca439a5b44113a6e48fb6933699a65b0a282b4d8870c29e5c26ad40faf";
    let key = rendezvous_key_for_topic(&format!("group/{hex}")).expect("group key");
    assert_eq!(key, *format!("/calimero/grp/{hex}").as_str());
}

#[test]
fn test_rendezvous_key_for_bare_context_topic() {
    // A bare topic is a context id (bs58). The network layer treats it
    // opaquely; both sides hold the identical string so the keys match.
    let ctx = "3iBu7jgK54DETcmDzrJbwtexC74ykBpH84aRG6Hpvjqs";
    let key = rendezvous_key_for_topic(ctx).expect("context key");
    assert_eq!(key, *format!("/calimero/ctx/{ctx}").as_str());
}

#[test]
fn test_rendezvous_key_distinct_per_kind_even_for_same_id() {
    // Same hex under ns/ vs group/ must NOT collide — the prefix
    // disambiguates, otherwise a namespace and a group with the same id
    // would share a rendezvous key and cross-pollute discovery.
    let hex = "aa".repeat(32);
    let ns = rendezvous_key_for_topic(&format!("ns/{hex}")).unwrap();
    let grp = rendezvous_key_for_topic(&format!("group/{hex}")).unwrap();
    assert_ne!(ns, grp, "ns and group keys must differ for the same id");
}

#[test]
fn test_rendezvous_key_rejects_overlong_topic() {
    // A pathological topic that would push the key past the 255-char
    // rendezvous limit is dropped (None) rather than panicking.
    let huge = "z".repeat(300);
    assert!(
        rendezvous_key_for_topic(&huge).is_none(),
        "over-length topic must map to None, not panic"
    );
}

// ---------------------------------------------------------------------------
// under_connected_rendezvous_keys — demand-driven discovery selection
// ---------------------------------------------------------------------------

#[test]
fn test_under_connected_includes_only_zero_subscriber_topics() {
    let starved = "ns/aa";
    let healthy = "ns/bb";
    let keys = under_connected_rendezvous_keys([(starved, 0), (healthy, 3)]);
    assert_eq!(
        keys,
        vec![rendezvous_key_for_topic(starved).unwrap()],
        "only the zero-mesh-peer topic should be selected for discovery"
    );
}

#[test]
fn test_under_connected_empty_when_all_healthy() {
    let keys = under_connected_rendezvous_keys([("ns/aa", 1), ("group/bb", 2), ("ctxid", 5)]);
    assert!(
        keys.is_empty(),
        "no discovery load when every overlay already has a connected peer"
    );
}

#[test]
fn test_under_connected_dedups_repeated_keys() {
    // Two identical topics (or any that map alike) collapse to one key so
    // we don't issue duplicate discover requests in the same pass.
    let keys = under_connected_rendezvous_keys([("ns/aa", 0), ("ns/aa", 0)]);
    assert_eq!(keys.len(), 1, "duplicate keys must be collapsed");
}

#[test]
fn test_under_connected_preserves_first_appearance_order() {
    let keys = under_connected_rendezvous_keys([("ns/aa", 0), ("group/bb", 0), ("ctxid", 0)]);
    assert_eq!(
        keys,
        vec![
            rendezvous_key_for_topic("ns/aa").unwrap(),
            rendezvous_key_for_topic("group/bb").unwrap(),
            rendezvous_key_for_topic("ctxid").unwrap(),
        ],
        "order should follow first appearance for predictable round-robin pacing"
    );
}

#[test]
fn test_rendezvous_key_max_length_boundary() {
    // Ensure we accept a valid key up to the 255-char rendezvous limit.
    // A namespace prefix "/calimero/ns/" is 13 chars, leaving 242 chars for the id.
    // Use exactly 242 hex chars to hit the boundary without exceeding it.
    let hex_242 = "a".repeat(242);
    let topic = format!("ns/{hex_242}");
    let key = rendezvous_key_for_topic(&topic);
    assert!(
        key.is_some(),
        "maximum-length valid topic (255 chars total) must be accepted"
    );
    assert_eq!(
        key.unwrap(),
        *format!("/calimero/ns/{hex_242}").as_str(),
        "key derivation must match the composed prefix + id"
    );
}

#[test]
fn test_under_connected_mixed_healthy_and_starved_with_order_and_dedup() {
    // A mix of topics with varying mesh_peer_count: some healthy (>0),
    // some starved (0). Must return only the starved ones, in first-appearance
    // order, with duplicates collapsed.
    let topics = [
        ("ns/aa", 1),    // healthy, skip
        ("group/bb", 0), // starved, include (first appearance)
        ("ctxid", 2),    // healthy, skip
        ("ns/cc", 0),    // starved, include
        ("group/bb", 0), // starved, duplicate of "group/bb", skip
        ("ns/aa", 0), // starved, but first appearance as "ns/aa" was healthy; include new zero entry
    ];

    let keys = under_connected_rendezvous_keys(topics);

    // Expected: group/bb (first appearance with 0), ns/cc (first starved ns topic),
    // then ns/aa as starved (the topic's final state is 0, but its first appearance was
    // healthy at index 0, so this is the starved appearance at index 5).
    assert_eq!(
        keys.len(),
        3,
        "should include all distinct starved keys, filtering out healthy ones"
    );

    // Verify order follows first *starved* appearance, not topic name order:
    assert_eq!(
        keys[0],
        rendezvous_key_for_topic("group/bb").unwrap(),
        "group/bb should appear first (first starved appearance at index 1)"
    );
    assert_eq!(
        keys[1],
        rendezvous_key_for_topic("ns/cc").unwrap(),
        "ns/cc should appear second (second starved appearance at index 3)"
    );
    assert_eq!(
        keys[2],
        rendezvous_key_for_topic("ns/aa").unwrap(),
        "ns/aa should appear third (first time it's starved, at index 5)"
    );
}

#[test]
fn test_under_connected_empty_input() {
    // Calling with an empty topic list must return an empty result, not panic.
    let keys = under_connected_rendezvous_keys(vec![]);
    assert!(
        keys.is_empty(),
        "empty input must produce empty output, not panic or loop forever"
    );
}

#[test]
fn test_has_regular_connected_peer_multiple_regulars() {
    // Multiple regular peers + infra peers: should be true if ANY regular is present.
    let mut state = DiscoveryState::default();
    let relay = PeerId::random();
    let rendezvous = PeerId::random();
    let regular_a = PeerId::random();
    let regular_b = PeerId::random();

    state.update_peer_protocols(&relay, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&rendezvous, &[RENDEZVOUS_PROTOCOL_NAME]);
    // regular_a and regular_b are never marked as relay/rendezvous

    let connected = [relay, rendezvous, regular_a, regular_b];
    assert!(
        state.has_regular_connected_peer(connected.iter()),
        "multiple regular peers alongside infra should return true"
    );
}

#[test]
fn test_has_regular_connected_peer_many_infra_one_regular() {
    // An edge case: many infrastructure peers (relay + rendezvous + more),
    // but exactly one regular peer hidden among them. Must still return true.
    let mut state = DiscoveryState::default();
    let relay_1 = PeerId::random();
    let relay_2 = PeerId::random();
    let rendezvous_1 = PeerId::random();
    let rendezvous_2 = PeerId::random();
    let regular = PeerId::random();

    state.update_peer_protocols(&relay_1, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&relay_2, &[HOP_PROTOCOL_NAME]);
    state.update_peer_protocols(&rendezvous_1, &[RENDEZVOUS_PROTOCOL_NAME]);
    state.update_peer_protocols(&rendezvous_2, &[RENDEZVOUS_PROTOCOL_NAME]);
    // regular never marked

    let connected = [relay_1, rendezvous_1, relay_2, regular, rendezvous_2];
    assert!(
        state.has_regular_connected_peer(connected.iter()),
        "single regular among many infra peers must be detected"
    );
}

#[test]
fn test_rendezvous_key_group_and_context_produce_distinct_keys() {
    // The same hex id under group/ and a bare context should produce
    // different keys (group uses /calimero/grp/, context uses /calimero/ctx/).
    // This ensures a group and a context-topic with the same id don't collide.
    let id = "8a6157eacc0e68d1786a585891866794d6fc5c11a199dbcb81ed33f5759a37a1";
    let grp_key =
        rendezvous_key_for_topic(&format!("group/{id}")).expect("group key should be valid");
    let ctx_key = rendezvous_key_for_topic(id).expect("context key should be valid");

    assert_ne!(
        grp_key, ctx_key,
        "group and bare-context topics with the same id must produce distinct keys \
         to avoid cross-pollination"
    );
    assert!(grp_key.to_string().contains("/calimero/grp/"));
    assert!(ctx_key.to_string().contains("/calimero/ctx/"));
}
