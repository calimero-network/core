//! Per-namespace rendezvous round-trip against a real libp2p rendezvous
//! server, using the production [`calimero_network::behaviour::Behaviour`]
//! (rendezvous *client*) on both ends.
//!
//! Pins the keystone of the discovery rework: a node that registers under
//! a namespace-scoped key `/calimero/ns/<hex>` is discoverable by a
//! co-member querying that same key — and a peer querying a *different*
//! namespace key does NOT find it. That namespace isolation is what makes
//! `discover` return only relevant co-members instead of the whole
//! network (the per-namespace rendezvous design). The exact topic→key
//! derivation (`rendezvous_key_for_topic`) is unit-tested separately; here
//! we assert the wire-format round-trips and is isolated.

use core::time::Duration;

use calimero_network::behaviour::{Behaviour, BehaviourEvent};
use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig, RelayConfig,
    RendezvousConfig, SwarmConfig,
};
use futures_util::StreamExt;
use libp2p::identity::Keypair;
use libp2p::rendezvous::client::Event as RzClientEvent;
use libp2p::rendezvous::{server as rz_server, Namespace};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{identify, noise, tcp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder};
use tokio::time::timeout;

#[derive(NetworkBehaviour)]
struct RzServerBehaviour {
    rendezvous: rz_server::Behaviour,
    identify: identify::Behaviour,
}

fn client_config(keypair: Keypair, listen: Multiaddr) -> NetworkConfig {
    NetworkConfig::new(
        keypair,
        SwarmConfig::new(vec![listen]),
        BootstrapConfig::new(BootstrapNodes::new(vec![])),
        DiscoveryConfig::new(
            // mdns off so the only way peers learn each other is via the
            // rendezvous server — exactly what we're testing.
            false,
            false,
            Vec::new(),
            RendezvousConfig::default(),
            RelayConfig::default(),
            AutonatConfig::new(5, Duration::from_secs(10)),
        ),
    )
}

/// Build a TCP-transport rendezvous server (same transport as the client
/// `Behaviour`, so they interoperate) and return it plus its listen addr.
async fn build_server() -> (Swarm<RzServerBehaviour>, PeerId, Multiaddr) {
    let mut swarm = SwarmBuilder::with_new_identity()
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .unwrap()
        .with_behaviour(|key| RzServerBehaviour {
            rendezvous: rz_server::Behaviour::new(rz_server::Config::default()),
            identify: identify::Behaviour::new(identify::Config::new(
                "/calimero/test-rz/1.0.0".to_owned(),
                key.public(),
            )),
        })
        .unwrap()
        .build();
    let peer_id = *swarm.local_peer_id();
    swarm
        .listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap())
        .unwrap();
    let addr = loop {
        if let SwarmEvent::NewListenAddr { address, .. } = swarm.next().await.unwrap() {
            break address;
        }
    };
    (swarm, peer_id, addr)
}

async fn build_client(listen: Multiaddr) -> Swarm<Behaviour> {
    let mut swarm =
        Behaviour::build_swarm(&client_config(Keypair::generate_ed25519(), listen)).unwrap();
    // Register our concrete listen address as external so the rendezvous
    // `register` carries a real address in its signed record (otherwise it
    // fails with `NoExternalAddresses`).
    loop {
        let listeners: Vec<Multiaddr> = swarm.listeners().cloned().collect();
        if let Some(addr) = listeners.into_iter().next() {
            swarm.add_external_address(addr);
            break;
        }
        let _ = swarm.next().await;
    }
    swarm
}

fn ns(byte: u8) -> Namespace {
    Namespace::new(format!("/calimero/ns/{}", hex::encode([byte; 32]))).unwrap()
}

#[tokio::test]
async fn co_member_discovers_under_namespace_key_others_do_not() {
    let port_a = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let port_b = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };

    let (mut server, server_peer, server_addr) = build_server().await;
    let mut a = build_client(format!("/ip4/127.0.0.1/tcp/{port_a}").parse().unwrap()).await;
    let mut b = build_client(format!("/ip4/127.0.0.1/tcp/{port_b}").parse().unwrap()).await;
    let peer_a = *a.local_peer_id();

    let ns_team = ns(0x11); // the namespace A registers under
    let ns_other = ns(0x22); // an unrelated namespace

    a.dial(server_addr.clone()).expect("A dial server");
    b.dial(server_addr.clone()).expect("B dial server");

    // Drives all three swarms through: A connects → A registers under
    // ns_team → B discovers ns_other (must NOT find A) → B discovers
    // ns_team (must find A).
    #[derive(PartialEq)]
    enum Phase {
        Registering,
        NegativeQuery,
        PositiveQuery,
    }
    let mut phase = Phase::Registering;
    let mut a_connected = false;
    let mut b_connected = false;
    let mut a_register_sent = false;

    let outcome = timeout(Duration::from_secs(45), async {
        loop {
            tokio::select! {
                ev = server.next() => { let _ = ev; }
                ev = a.next() => {
                    if let Some(SwarmEvent::ConnectionEstablished { peer_id, .. }) = ev {
                        if peer_id == server_peer { a_connected = true; }
                    }
                    if let Some(SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        RzClientEvent::Registered { .. },
                    ))) = ev
                    {
                        // A is now discoverable under ns_team; start B's
                        // negative query (a different namespace).
                        if b_connected {
                            b.behaviour_mut()
                                .rendezvous
                                .discover(Some(ns_other.clone()), None, None, server_peer);
                            phase = Phase::NegativeQuery;
                        }
                    }
                }
                ev = b.next() => {
                    if let Some(SwarmEvent::ConnectionEstablished { peer_id, .. }) = ev {
                        if peer_id == server_peer { b_connected = true; }
                    }
                    if let Some(SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        RzClientEvent::Discovered { registrations, .. },
                    ))) = ev
                    {
                        let found_a = registrations
                            .iter()
                            .any(|r| r.record.peer_id() == peer_a);
                        match phase {
                            Phase::NegativeQuery => {
                                assert!(
                                    !found_a,
                                    "A must NOT be discoverable under an unrelated namespace key"
                                );
                                // Now query the namespace A actually registered under.
                                b.behaviour_mut().rendezvous.discover(
                                    Some(ns_team.clone()),
                                    None,
                                    None,
                                    server_peer,
                                );
                                phase = Phase::PositiveQuery;
                            }
                            Phase::PositiveQuery => {
                                if found_a {
                                    return; // success
                                }
                            }
                            Phase::Registering => {}
                        }
                    }
                }
            }

            // Once A is connected, register it under ns_team (one-shot).
            if a_connected && !a_register_sent {
                a.behaviour_mut()
                    .rendezvous
                    .register(ns_team.clone(), server_peer, None)
                    .expect("A register");
                a_register_sent = true;
            }
        }
    })
    .await;

    outcome.expect("rendezvous round-trip did not complete (A registered, B should find it under ns_team but not ns_other)");
}

#[tokio::test]
async fn client_discovered_under_global_and_per_namespace_keys_additive() {
    let port_a = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };
    let port_b = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        p
    };

    let (mut server, server_peer, server_addr) = build_server().await;
    let mut a = build_client(format!("/ip4/127.0.0.1/tcp/{port_a}").parse().unwrap()).await;
    let mut b = build_client(format!("/ip4/127.0.0.1/tcp/{port_b}").parse().unwrap()).await;
    let peer_a = *a.local_peer_id();

    // A registers under BOTH:
    // 1. the global namespace (from RendezvousConfig::default())
    // 2. a per-namespace key (e.g., a topic/overlay A belongs to)
    let global_ns = Namespace::from_static("/calimero/devnet/global");
    let per_ns = ns(0x33); // A's per-overlay namespace

    a.dial(server_addr.clone()).expect("A dial server");
    b.dial(server_addr.clone()).expect("B dial server");

    // Phase machine:
    // RegisterGlobal: A registers under global namespace
    // RegisterPerNs: A registers under per-namespace key
    // DiscoverGlobal: B queries global namespace (must find A)
    // DiscoverPerNs: B queries per-namespace key (must also find A)
    #[derive(PartialEq)]
    enum Phase {
        RegisterGlobal,
        RegisterPerNs,
        DiscoverGlobal,
        DiscoverPerNs,
    }
    let mut phase = Phase::RegisterGlobal;
    let mut a_connected = false;
    let mut b_connected = false;
    let mut a_global_sent = false;
    let mut a_per_ns_sent = false;
    let mut found_a_global = false;
    let mut found_a_per_ns = false;

    let outcome = timeout(Duration::from_secs(45), async {
        loop {
            tokio::select! {
                ev = server.next() => { let _ = ev; }
                ev = a.next() => {
                    if let Some(SwarmEvent::ConnectionEstablished { peer_id, .. }) = ev {
                        if peer_id == server_peer { a_connected = true; }
                    }
                    if let Some(SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        RzClientEvent::Registered { .. },
                    ))) = ev
                    {
                        // After A registers under a key, move to the next phase.
                        match phase {
                            Phase::RegisterGlobal => {
                                a_global_sent = true;
                                phase = Phase::RegisterPerNs;
                            }
                            Phase::RegisterPerNs => {
                                a_per_ns_sent = true;
                                // Both registers done; now B can start discovering.
                                if b_connected {
                                    b.behaviour_mut().rendezvous.discover(
                                        Some(global_ns.clone()),
                                        None,
                                        None,
                                        server_peer,
                                    );
                                    phase = Phase::DiscoverGlobal;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                ev = b.next() => {
                    if let Some(SwarmEvent::ConnectionEstablished { peer_id, .. }) = ev {
                        if peer_id == server_peer { b_connected = true; }
                    }
                    if let Some(SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        RzClientEvent::Discovered { registrations, .. },
                    ))) = ev
                    {
                        let found_a = registrations
                            .iter()
                            .any(|r| r.record.peer_id() == peer_a);
                        match phase {
                            Phase::DiscoverGlobal => {
                                if found_a {
                                    found_a_global = true;
                                    // Now discover A under the per-namespace key.
                                    b.behaviour_mut().rendezvous.discover(
                                        Some(per_ns.clone()),
                                        None,
                                        None,
                                        server_peer,
                                    );
                                    phase = Phase::DiscoverPerNs;
                                }
                            }
                            Phase::DiscoverPerNs => {
                                if found_a {
                                    found_a_per_ns = true;
                                    return; // success
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            // Phase state machine for A's registrations.
            if a_connected {
                if !a_global_sent {
                    a.behaviour_mut()
                        .rendezvous
                        .register(global_ns.clone(), server_peer, None)
                        .expect("A register under global");
                    a_global_sent = true;
                } else if !a_per_ns_sent && phase == Phase::RegisterPerNs {
                    a.behaviour_mut()
                        .rendezvous
                        .register(per_ns.clone(), server_peer, None)
                        .expect("A register under per-namespace");
                    a_per_ns_sent = true;
                }
            }
        }
    })
    .await;

    outcome.expect(
        "A must register under BOTH global and per-namespace keys, \
         and B must discover A under both (additive registration)",
    );
    assert!(
        found_a_global,
        "A must be discoverable under global namespace"
    );
    assert!(
        found_a_per_ns,
        "A must be discoverable under per-namespace key"
    );
}
