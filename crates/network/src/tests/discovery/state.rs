use libp2p::rendezvous::Namespace;

use super::*;

#[test]
fn test_get_preferred_addr() {
    let mut peer_info = PeerInfo::default();
    let tcp_addr_1: Multiaddr = "/ip4/127.0.0.1/tcp/4001".parse().unwrap();
    let tcp_addr_2: Multiaddr = "/ip4/127.0.0.1/tcp/4002".parse().unwrap();
    let quic_addr: Multiaddr = "/ip4/127.0.0.1/udp/4001".parse().unwrap();

    assert_eq!(peer_info.get_preferred_addr(), None);

    let _ = peer_info.addrs.insert(tcp_addr_1.clone());
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Tcp(4001)
    );

    let _ = peer_info.addrs.insert(quic_addr.clone());
    assert_eq!(
        peer_info
            .get_preferred_addr()
            .unwrap()
            .clone()
            .pop()
            .unwrap(),
        Protocol::Udp(4001)
    );

    let _ = peer_info.addrs.insert(tcp_addr_2.clone());
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
fn test_is_relay_reservation_required() {
    let mut peer_info = PeerInfo::default();
    assert_eq!(peer_info.is_relay_reservation_required(), true);

    peer_info.relay = Some(PeerRelayInfo {
        reservation_status: RelayReservationStatus::Requested,
    });
    assert_eq!(peer_info.is_relay_reservation_required(), false);

    peer_info.relay = Some(PeerRelayInfo {
        reservation_status: RelayReservationStatus::Accepted,
    });
    assert_eq!(peer_info.is_relay_reservation_required(), false);

    peer_info.relay = Some(PeerRelayInfo {
        reservation_status: RelayReservationStatus::Discovered,
    });
    assert_eq!(peer_info.is_relay_reservation_required(), true);

    peer_info.relay = Some(PeerRelayInfo {
        reservation_status: RelayReservationStatus::Expired,
    });
    assert_eq!(peer_info.is_relay_reservation_required(), true);
}

#[test]
fn test_is_rendezvous_discovery_throttled() {
    let mut peer_info = PeerInfo::default();
    assert_eq!(peer_info.is_rendezvous_discover_throttled(1.0), false);

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        last_discovery_at: Some(Instant::now() - Duration::from_secs(30)),
        ..Default::default()
    });
    assert_eq!(peer_info.is_rendezvous_discover_throttled(1.0), true);

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        last_discovery_at: Some(Instant::now() - Duration::from_secs(61)),
        ..Default::default()
    });
    assert_eq!(peer_info.is_rendezvous_discover_throttled(1.0), false);
}

#[test]
fn test_is_rendezvous_registration_required() {
    let mut peer_info = PeerInfo::default();
    assert_eq!(peer_info.is_rendezvous_registration_required(), true);

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        registration_status: RendezvousRegistrationStatus::Requested,
        ..Default::default()
    });
    assert_eq!(peer_info.is_rendezvous_registration_required(), false);

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        registration_status: RendezvousRegistrationStatus::Registered,
        ..Default::default()
    });
    assert_eq!(peer_info.is_rendezvous_registration_required(), false);

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        registration_status: RendezvousRegistrationStatus::Discovered,
        ..Default::default()
    });
    assert_eq!(peer_info.is_rendezvous_registration_required(), true);

    peer_info.rendezvous = Some(PeerRendezvousInfo {
        registration_status: RendezvousRegistrationStatus::Expired,
        ..Default::default()
    });
    assert_eq!(peer_info.is_rendezvous_registration_required(), true);
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
    assert_eq!(state.is_peer_relay(&peer_id), true);
    assert_eq!(state.is_peer_rendezvous(&peer_id), true);
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

    state.add_peer_addr(peer_id.clone(), &quic_addr);
    state.add_peer_addr(peer_id.clone(), &tcp_addr);
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
