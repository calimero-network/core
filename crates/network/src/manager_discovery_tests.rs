//! End-to-end discovery tests that drive the REAL [`NetworkManager`] —
//! production event handlers, cookie lifecycle, identify-triggered
//! register/discover — against a real libp2p rendezvous server, all
//! in-process.
//!
//! Why this exists: the merobox e2e fleet runs every node on one Docker
//! network with direct bootstrap, so the rendezvous discovery layer is
//! dead code in every CI scenario — and the crate's integration tests
//! call `behaviour_mut().rendezvous.discover(...)` directly, bypassing
//! the manager's event handlers. That combination let the cookie-
//! poisoning bug (a per-overlay discover response's cookie stored into
//! the slot the global discover replays → `InvalidCookie` forever →
//! namespace-invite joins dead until restart) ship with CI fully green.
//!
//! Here the manager's own swarm is pumped through
//! [`NetworkManager::dispatch_behaviour_event`], so identify marks the
//! server as a rendezvous peer, triggers the first discover, and every
//! `Discovered`/`DiscoverFailed` flows through the same code a
//! production node runs. In-process TCP with mDNS off gives the
//! "isolated network" for free: rendezvous is the only way these peers
//! can find each other.

use core::time::Duration;
use std::collections::BTreeSet;
use std::sync::Arc;

use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig, RelayConfig,
    RendezvousConfig, SwarmConfig,
};
use calimero_network_primitives::messages::{NetworkEvent, NetworkEventDispatcher};
use futures_util::StreamExt;
use libp2p::identity::Keypair;
use libp2p::rendezvous::client::Event as RzClientEvent;
use libp2p::rendezvous::{server as rz_server, Cookie, ErrorCode, Namespace};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{gossipsub, identify, noise, tcp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder};
use prometheus_client::registry::Registry;
use tokio::time::timeout;

use crate::behaviour::{Behaviour, BehaviourEvent};
use crate::NetworkManager;

struct NoopDispatcher;

impl NetworkEventDispatcher for NoopDispatcher {
    fn dispatch(&self, _event: NetworkEvent) -> bool {
        true
    }
}

#[derive(NetworkBehaviour)]
struct RzServerBehaviour {
    rendezvous: rz_server::Behaviour,
    identify: identify::Behaviour,
}

/// TCP rendezvous server (same transport as the production client) plus
/// its listen address. Its identify announces `/rendezvous/1.0.0`, which
/// is what the manager's identify handler keys `is_peer_rendezvous` on.
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

fn client_config(listen: Multiaddr) -> NetworkConfig {
    NetworkConfig::new(
        Keypair::generate_ed25519(),
        SwarmConfig::new(vec![listen]),
        BootstrapConfig::new(BootstrapNodes::new(vec![])),
        DiscoveryConfig::new(
            // mdns off: rendezvous must be the only discovery path,
            // mirroring internet-separated nodes.
            false,
            false,
            Vec::new(),
            RendezvousConfig::default(),
            RelayConfig::default(),
            AutonatConfig::new(5, Duration::from_secs(10)),
        ),
    )
}

async fn free_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// A raw production-`Behaviour` swarm standing in for a node that is
/// already a member of the namespace (the inviter side). Registered
/// under the given keys before this returns, so the manager under test
/// has something to discover.
async fn build_registered_member(
    server: &mut Swarm<RzServerBehaviour>,
    server_peer: PeerId,
    server_addr: &Multiaddr,
    namespaces: &[Namespace],
) -> (Swarm<Behaviour>, PeerId) {
    let listen: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", free_port().await)
        .parse()
        .unwrap();
    let mut member = Behaviour::build_swarm(&client_config(listen)).unwrap();
    let member_peer = *member.local_peer_id();

    // Register a concrete external address so `register` carries a real
    // signed record (otherwise it fails with `NoExternalAddresses`).
    loop {
        let listeners: Vec<Multiaddr> = member.listeners().cloned().collect();
        if let Some(addr) = listeners.into_iter().next() {
            member.add_external_address(addr);
            break;
        }
        let _ = member.next().await;
    }

    member
        .dial(server_addr.clone())
        .expect("member dial server");

    let mut registered = 0_usize;
    let mut register_sent = false;
    timeout(Duration::from_secs(30), async {
        loop {
            tokio::select! {
                ev = server.next() => { let _ = ev; }
                ev = member.next() => {
                    if let Some(SwarmEvent::ConnectionEstablished { peer_id, .. }) = ev {
                        if peer_id == server_peer && !register_sent {
                            for ns in namespaces {
                                member
                                    .behaviour_mut()
                                    .rendezvous
                                    .register(ns.clone(), server_peer, None)
                                    .expect("member register");
                            }
                            register_sent = true;
                        }
                    }
                    if let Some(SwarmEvent::Behaviour(BehaviourEvent::Rendezvous(
                        RzClientEvent::Registered { .. },
                    ))) = ev
                    {
                        registered += 1;
                        if registered == namespaces.len() {
                            return;
                        }
                    }
                }
            }
        }
    })
    .await
    .expect("member did not finish registering");

    (member, member_peer)
}

fn overlay_topic_and_key(byte: u8) -> (String, Namespace) {
    let topic = format!("ns/{}", hex::encode([byte; 32]));
    let key = Namespace::new(format!("/calimero/ns/{}", hex::encode([byte; 32]))).unwrap();
    (topic, key)
}

async fn build_manager() -> NetworkManager {
    let listen: Multiaddr = format!("/ip4/127.0.0.1/tcp/{}", free_port().await)
        .parse()
        .unwrap();
    let mut registry = Registry::default();
    NetworkManager::new(
        &client_config(listen),
        Arc::new(NoopDispatcher),
        &mut registry,
        BTreeSet::new(),
        None,
    )
    .await
    .expect("manager builds without a store")
}

/// The regression that shipped as a production outage: after a tick
/// where BOTH the global and a per-overlay discover ran (the production
/// shape of `rendezvous_discover` once the node subscribes overlay
/// topics), the next global discover must still succeed. Pre-fix, the
/// per-overlay response's cookie was stored into the slot the global
/// discover replays and the server answered `InvalidCookie` — forever,
/// because nothing cleared it.
#[tokio::test]
async fn mixed_global_and_overlay_ticks_never_poison_the_global_cookie() {
    let (mut server, server_peer, server_addr) = build_server().await;

    let global_ns = Namespace::from_static("/calimero/devnet/global");
    let (overlay_topic, overlay_key) = overlay_topic_and_key(0x55);

    let (mut member, member_peer) = build_registered_member(
        &mut server,
        server_peer,
        &server_addr,
        &[global_ns.clone(), overlay_key.clone()],
    )
    .await;

    let mut manager = build_manager().await;

    // Subscribing the manager to the overlay topic (with zero connected
    // subscribers) is what makes `rendezvous_discover` send the
    // per-overlay discover alongside the global one — the exact traffic
    // mix that poisoned production nodes.
    let topic = gossipsub::IdentTopic::new(overlay_topic);
    let _ = manager
        .swarm
        .behaviour_mut()
        .gossipsub
        .subscribe(&topic)
        .expect("subscribe overlay topic");

    manager
        .swarm
        .dial(server_addr.clone())
        .expect("manager dial server");

    // Deterministic sequence (no dependence on response ordering):
    //   1. identify (auto) → first global discover (cookie-less) →
    //      global cookie stored.
    //   2. one per-overlay discover (the cookie-less leg
    //      `rendezvous_discover` sends for every under-connected
    //      overlay key) → its response is the ONLY discover in flight,
    //      so whatever the handler does with its cookie is exactly
    //      what's in the slot next.
    //   3. forced production tick → the global leg replays the stored
    //      cookie. Pre-fix that is the overlay cookie (stored in step
    //      2) and the server answers `InvalidCookie`; post-fix the
    //      handler never stored it, the global cookie survives, and
    //      the discover succeeds.
    let mut global_discovers_seen = 0_usize;
    let mut overlay_discover_seen = false;
    let mut found_member = false;
    let mut phase = 0_usize;

    timeout(Duration::from_secs(45), async {
        loop {
            tokio::select! {
                ev = server.next() => { let _ = ev; }
                ev = member.next() => { let _ = ev; }
                ev = manager.swarm.next() => {
                    let Some(ev) = ev else { continue };
                    if let SwarmEvent::Behaviour(behaviour_ev) = ev {
                        match &behaviour_ev {
                            BehaviourEvent::Rendezvous(RzClientEvent::Discovered {
                                registrations,
                                cookie,
                                ..
                            }) => {
                                if registrations
                                    .iter()
                                    .any(|r| r.record.peer_id() == member_peer)
                                {
                                    found_member = true;
                                }
                                if cookie.namespace() == Some(&global_ns) {
                                    global_discovers_seen += 1;
                                } else if cookie.namespace() == Some(&overlay_key) {
                                    overlay_discover_seen = true;
                                }
                            }
                            BehaviourEvent::Rendezvous(RzClientEvent::DiscoverFailed {
                                error,
                                ..
                            }) => {
                                panic!(
                                    "discover failed with {error:?} — a stored cookie was \
                                     replayed against the wrong namespace (the production \
                                     cookie-poisoning bug)"
                                );
                            }
                            _ => {}
                        }
                        // The REAL handler: stores/skips the cookie,
                        // triggers identify-driven discovery, dials
                        // discovered peers.
                        manager.dispatch_behaviour_event(behaviour_ev);
                    }

                    if global_discovers_seen >= 1 && phase == 0 {
                        // Step 2: the per-overlay leg, exactly as
                        // `rendezvous_discover` sends it (cookie-less,
                        // namespaced to the overlay key). Issued alone so
                        // its response deterministically decides the slot.
                        manager.swarm.behaviour_mut().rendezvous.discover(
                            Some(overlay_key.clone()),
                            None,
                            None,
                            server_peer,
                        );
                        phase = 1;
                    } else if overlay_discover_seen && phase == 1 {
                        // Step 3: full production tick — the global leg
                        // replays whatever cookie survived step 2.
                        // Pre-fix code detonates here with InvalidCookie.
                        manager
                            .rendezvous_discover(&server_peer, true)
                            .expect("forced tick");
                        phase = 2;
                    } else if global_discovers_seen >= 2 && phase == 2 {
                        return;
                    }
                }
            }
        }
    })
    .await
    .expect(
        "three healthy global discovers interleaved with a per-overlay discover \
         did not complete — global discovery died after the mixed tick",
    );

    assert!(found_member, "global discovery must surface the member");
    assert!(
        overlay_discover_seen,
        "the per-overlay discover must have run"
    );

    // The slot the next global discover replays must still hold a
    // global-namespace cookie, not the overlay one.
    let stored = manager
        .discovery
        .state
        .get_peer_info(&server_peer)
        .and_then(|info| info.rendezvous())
        .and_then(|rz| rz.cookie())
        .cloned()
        .expect("a cookie must be stored after successful global discovers");
    assert_eq!(stored.namespace(), Some(&global_ns));
}

/// The self-heal path, through the real `DiscoverFailed` handler: when
/// the server rejects the stored cookie (`InvalidCookie` — stale after a
/// server restart, or poisoned by a bug), the handler must clear it and
/// immediately re-discover cookie-less, restoring discovery without a
/// node restart. Pre-fix, the handler only logged a warning and the node
/// replayed the rejected cookie every tick until reboot.
#[tokio::test]
async fn rejected_cookie_self_heals_through_the_real_handler() {
    let (mut server, server_peer, server_addr) = build_server().await;

    let global_ns = Namespace::from_static("/calimero/devnet/global");
    let (_overlay_topic, overlay_key) = overlay_topic_and_key(0x66);

    let (mut member, member_peer) =
        build_registered_member(&mut server, server_peer, &server_addr, &[global_ns.clone()]).await;

    let mut manager = build_manager().await;
    manager
        .swarm
        .dial(server_addr.clone())
        .expect("manager dial server");

    let mut healthy_discovers = 0_usize;
    let mut poisoned = false;
    let mut saw_invalid_cookie = false;

    timeout(Duration::from_secs(45), async {
        loop {
            tokio::select! {
                ev = server.next() => { let _ = ev; }
                ev = member.next() => { let _ = ev; }
                ev = manager.swarm.next() => {
                    let Some(ev) = ev else { continue };
                    if let SwarmEvent::Behaviour(behaviour_ev) = ev {
                        match &behaviour_ev {
                            BehaviourEvent::Rendezvous(RzClientEvent::Discovered {
                                registrations, ..
                            }) => {
                                if registrations
                                    .iter()
                                    .any(|r| r.record.peer_id() == member_peer)
                                {
                                    healthy_discovers += 1;
                                }
                            }
                            BehaviourEvent::Rendezvous(RzClientEvent::DiscoverFailed {
                                error,
                                ..
                            }) => {
                                assert!(
                                    poisoned,
                                    "only the deliberately poisoned discover may fail"
                                );
                                assert_eq!(*error, ErrorCode::InvalidCookie);
                                saw_invalid_cookie = true;
                            }
                            _ => {}
                        }
                        // The real handler: on InvalidCookie it must
                        // clear the slot and force a cookie-less
                        // re-discover.
                        manager.dispatch_behaviour_event(behaviour_ev);

                        if saw_invalid_cookie {
                            assert!(
                                manager
                                    .discovery
                                    .state
                                    .get_peer_info(&server_peer)
                                    .and_then(|info| info.rendezvous())
                                    .and_then(|rz| rz.cookie())
                                    .is_none()
                                    || healthy_discovers >= 2,
                                "the rejected cookie must be cleared before (or replaced by) \
                                 the recovery discover"
                            );
                        }
                    }

                    if healthy_discovers >= 1 && !poisoned {
                        // Simulate the slot being poisoned (what the old
                        // Discovered handler did with a per-overlay
                        // cookie) — then let the production tick replay
                        // it against the global namespace.
                        let bad = Cookie::for_namespace(overlay_key.clone());
                        manager
                            .discovery
                            .state
                            .update_rendezvous_cookie(&server_peer, &bad);
                        poisoned = true;
                        manager
                            .rendezvous_discover(&server_peer, true)
                            .expect("poisoned tick");
                    } else if saw_invalid_cookie && healthy_discovers >= 2 {
                        // The handler's forced re-discover already ran
                        // cookie-less and found the member again:
                        // discovery recovered with no restart.
                        return;
                    }
                }
            }
        }
    })
    .await
    .expect(
        "discovery did not recover after InvalidCookie — the rejected cookie \
         was replayed instead of cleared",
    );

    assert!(saw_invalid_cookie);
}
