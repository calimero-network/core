use libp2p::rendezvous::Namespace;

use super::*;

#[test]
fn test_get_preferred_addr() {
    let mut peer_info = PeerInfo::default();
    let tcp_addr_1: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let tcp_addr_2: Multiaddr = "/ip4/127.0.0.1/tcp/4002".parse().unwrap();
    let quic_addr: Multiaddr = "/ip4/127.0.0.1/udp/4001".parse().unwrap();

    assert_eq!(peer_info.get_preferred_addr(), None);

    let _ = peer_info.addrs.insert(tcp_addr_1);
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Tcp(4001)
    );

    let _ = peer_info.addrs.insert(quic_addr);
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Udp(4001)
    );

    let _ = peer_info.addrs.insert(tcp_addr_2);
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
    assert!(state.peers[&peer_id].addrs.contains(&tcp_addr));
    assert!(state.peers[&peer_id].addrs.contains(&quic_addr));

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
