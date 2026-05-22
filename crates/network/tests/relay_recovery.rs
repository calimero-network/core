//! Integration tests for the relay-reservation recovery code paths in
//! `calimero-network`.
//!
//! These tests use [`common::mock_relay::MockRelay`] to stand up a
//! controllable libp2p relay server in the same process and drive the
//! production [`calimero_network::behaviour::Behaviour`] against it.
//!
//! ## What is tested here vs elsewhere
//!
//! These tests verify the **libp2p mechanics** that the recovery code
//! depends on:
//!   - that a real reservation can be set up against the mock relay
//!     (happy path),
//!   - that disconnecting the relay produces `SwarmEvent::ConnectionClosed`
//!     on the client with `num_established == 0` (the trigger condition for
//!     our `ConnectionClosed` recovery branch),
//!   - that a relay over its reservation quota produces `ListenerClosed`
//!     on the additional client (the trigger for the `ListenerClosed`
//!     recovery branch).
//!
//! The **state-machine handling** of those events (mark Expired, queue
//! recovery action, idempotency) is covered by the
//! `test_on_relay_reservation_lost_*` unit tests in
//! `crates/network/src/discovery/state_tests.rs`.
//!
//! Tests here intentionally do not spawn the `NetworkManager` actor, so they
//! don't directly verify the event router glue. That glue is mechanical
//! (single match arm + method call) and adding an actor harness here would
//! roughly double the size of this PR. Unit tests on the state machine plus
//! integration tests on the libp2p events give strong coverage of the bug;
//! a NetworkManager-actor harness can come as a follow-up if richer
//! end-to-end assertions are needed.
//!
//! ## Scenarios from the disconnect analysis
//!
//! - **Covered here**: relay control-connection drop (boot-node restart,
//!   sleep/wake, App Nap freeze, abrupt TCP reset), relay quota exhaustion
//!   (S6), happy-path reservation.
//!
//! - **Not covered here**: long-session renewal blip (S5) — the libp2p
//!   relay client auto-renews aggressively so a short server-side
//!   `reservation_duration` alone does not drive the client into
//!   `ExternalAddrExpired`; would need the relay to actively refuse a
//!   renewal, which is more MockRelay infrastructure than this PR adds.
//!   Client IP change mid-session (network switch, VPN toggle) and true
//!   symmetric NAT — hard to simulate inside a single kernel without root
//!   or multiple loopback interfaces. All of these are tracked in
//!   issue #2447 for future container- or netns-based tests.

mod common;

use core::time::Duration;
use std::sync::Arc;

use calimero_network::behaviour::{Behaviour, BehaviourEvent};
use calimero_network_primitives::config::{
    AutonatConfig, BootstrapConfig, BootstrapNodes, DiscoveryConfig, NetworkConfig, RelayConfig,
    RendezvousConfig, SwarmConfig,
};
use common::mock_relay::{MockRelay, MockRelayConfig};
use futures_util::StreamExt;
use libp2p::identity::Keypair;
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, Swarm};
use multiaddr::Protocol;
use tokio::time::timeout;

/// Build a NetworkConfig for a client node that will use the supplied
/// bootstrap addresses (typically a MockRelay).
fn client_config(keypair: Keypair, listen: Multiaddr, bootstrap: Vec<Multiaddr>) -> NetworkConfig {
    NetworkConfig::new(
        keypair,
        SwarmConfig::new(vec![listen]),
        BootstrapConfig::new(BootstrapNodes::new(bootstrap)),
        DiscoveryConfig::new(
            false,
            false,
            RendezvousConfig::default(),
            RelayConfig::default(),
            AutonatConfig::new(5, Duration::from_secs(10)),
        ),
    )
}

/// Loopback multiaddr with port 0. libp2p picks the actual port at bind
/// time, which avoids the TOCTOU race of pre-binding a TcpListener to find
/// a free port and then handing the address to libp2p (another process
/// could claim it in the gap, especially under parallel test execution).
fn ephemeral_listen_addr() -> Multiaddr {
    "/ip4/127.0.0.1/tcp/0".parse().unwrap()
}

/// Drive a client swarm until a predicate over its SwarmEvent stream returns
/// Some(T), or the timeout elapses.
async fn wait_for<F, T>(
    swarm: &mut Swarm<Behaviour>,
    label: &str,
    deadline: Duration,
    mut predicate: F,
) -> T
where
    F: FnMut(&SwarmEvent<BehaviourEvent>) -> Option<T>,
{
    let result = timeout(deadline, async {
        loop {
            let event = swarm
                .next()
                .await
                .unwrap_or_else(|| panic!("swarm stream closed while waiting for: {label}"));
            if let Some(value) = predicate(&event) {
                return value;
            }
        }
    })
    .await;

    result.unwrap_or_else(|_| panic!("timed out waiting for: {label}"))
}

/// True if `addr` is a relayed multiaddr (contains the p2p-circuit protocol).
fn is_relayed(addr: &Multiaddr) -> bool {
    addr.iter().any(|p| matches!(p, Protocol::P2pCircuit))
}

/// Drive a client swarm against the relay until a relayed listen address is
/// confirmed externally. Returns the local PeerId. Encapsulates the boring
/// dial-then-listen-on-circuit dance every scenario test needs.
async fn establish_reservation(
    client_swarm: &mut Swarm<Behaviour>,
    relay: &MockRelay,
) -> libp2p::PeerId {
    let local_peer_id = *client_swarm.local_peer_id();

    client_swarm
        .dial(relay.bootstrap_addr())
        .expect("dial mock relay");

    // Wait for the connection to the relay to be established. We don't need
    // to wait for identify to complete — the relay's reservation response
    // uses its OWN external addresses (which MockRelay sets at spawn time),
    // not anything we send via identify.
    wait_for(
        client_swarm,
        "ConnectionEstablished to mock relay",
        Duration::from_secs(10),
        |event| match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } if *peer_id == relay.peer_id() => {
                Some(())
            }
            _ => None,
        },
    )
    .await;

    let relayed_addr = relay
        .bootstrap_addr()
        .with(Protocol::P2pCircuit)
        .with(Protocol::P2p(local_peer_id));
    client_swarm
        .listen_on(relayed_addr)
        .expect("listen on relayed addr");

    wait_for(
        client_swarm,
        "ExternalAddrConfirmed with /p2p-circuit",
        Duration::from_secs(15),
        |event| match event {
            SwarmEvent::ExternalAddrConfirmed { address } if is_relayed(address) => Some(()),
            _ => None,
        },
    )
    .await;

    local_peer_id
}

/// End-to-end: a client with the production Behaviour, pointed at a fresh
/// MockRelay as its sole bootstrap, successfully reserves a relayed circuit.
///
/// Baseline test for the suite — if this doesn't pass, no scenario test that
/// depends on observing the reservation lifecycle can.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_can_reserve_circuit_against_mock_relay() {
    let relay = Arc::new(MockRelay::spawn().await.expect("spawn mock relay"));

    let client_keypair = Keypair::generate_ed25519();
    let client_listen = ephemeral_listen_addr();
    let mut client_swarm = Behaviour::build_swarm(&client_config(
        client_keypair,
        client_listen,
        vec![relay.bootstrap_addr()],
    ))
    .expect("build client swarm");

    let _ = establish_reservation(&mut client_swarm, &relay).await;

    let obs = relay.observations().await;
    assert!(
        obs.reservations_accepted >= 1,
        "relay should have accepted at least one reservation; got {obs:?}"
    );

    relay.shutdown().await;
}

/// Scenario: relay process restarts (or any other event that drops the
/// control connection — sleep/wake, App Nap freeze, abrupt TCP reset).
///
/// Mocked by [`MockRelay::disconnect_peer`], which closes the control
/// connection from the relay side. The client's libp2p stack reports
/// SwarmEvent::ConnectionClosed for the relay peer, which is the trigger
/// our recovery code in `handlers/stream/swarm.rs` reacts to.
///
/// **What this test asserts**: ConnectionClosed actually fires for the
/// relay peer when the relay disconnects, with no remaining connections.
/// The state-machine handling of that event is covered by the
/// `test_on_relay_reservation_lost_*` unit tests; this verifies that the
/// libp2p event our handler is wired to fires in the situation we care about.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_disconnect_triggers_client_connection_closed() {
    let relay = Arc::new(MockRelay::spawn().await.expect("spawn mock relay"));
    let relay_peer_id = relay.peer_id();

    let client_keypair = Keypair::generate_ed25519();
    let client_listen = ephemeral_listen_addr();
    let mut client_swarm = Behaviour::build_swarm(&client_config(
        client_keypair,
        client_listen,
        vec![relay.bootstrap_addr()],
    ))
    .expect("build client swarm");

    let local_peer_id = establish_reservation(&mut client_swarm, &relay).await;

    // Cause the relay to disconnect the client. Mirrors a boot-node going down.
    let disconnected = relay.disconnect_peer(local_peer_id).await;
    assert!(
        disconnected,
        "mock relay should have had a live connection to the client to disconnect"
    );

    // The client must observe ConnectionClosed for the relay peer with
    // num_established == 0. This is the signal our ConnectionClosed handler
    // in handlers/stream/swarm.rs uses to call on_relay_reservation_lost.
    wait_for(
        &mut client_swarm,
        "ConnectionClosed for relay peer",
        Duration::from_secs(10),
        |event| match event {
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established,
                ..
            } if *peer_id == relay_peer_id && *num_established == 0 => Some(()),
            _ => None,
        },
    )
    .await;

    relay.shutdown().await;
}

/// Scenario: relay refuses a reservation request because the per-peer quota
/// is already full. Real-world trigger: a burst of new clients hits the
/// boot-node at the same time and one of them loses the race for the last
/// open slot, or a client's existing reservation has counted toward the
/// per-peer limit when it tries to take a second one.
///
/// Mocked by setting `max_reservations_per_peer = 1` and having the client
/// try to take a second reservation on the same relay. The relay's
/// `relay::Behaviour` responds with `RESOURCE_LIMIT_EXCEEDED`, which the
/// libp2p relay client surfaces by tearing down the listener.
///
/// **What this test asserts**: `ListenerClosed` fires on the client with a
/// relayed address in the closed listener's addresses (or an empty
/// `addresses` list, depending on whether the reservation got an address
/// allocated before denial). This is the trigger for the `ListenerClosed`
/// branch of recovery in `handlers/stream/swarm.rs`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_quota_exhaustion_denies_second_client() {
    // Total quota = 1. First client succeeds; second client must be denied
    // and observe a ListenerClosed (or never see ExternalAddrConfirmed
    // within the deadline). The denial is the path that, in production, our
    // ListenerClosed branch in handlers/stream/swarm.rs would react to.
    let relay_keypair = Keypair::generate_ed25519();
    let relay = Arc::new(
        MockRelay::spawn_with(
            MockRelayConfig {
                max_reservations: 1,
                max_reservations_per_peer: 1,
                ..MockRelayConfig::default()
            },
            relay_keypair,
        )
        .await
        .expect("spawn mock relay"),
    );

    // Client A: takes the only available slot.
    let mut client_a = Behaviour::build_swarm(&client_config(
        Keypair::generate_ed25519(),
        ephemeral_listen_addr(),
        vec![relay.bootstrap_addr()],
    ))
    .expect("build client A");
    let _ = establish_reservation(&mut client_a, &relay).await;

    // Drive client A's swarm continuously in a background task so its
    // libp2p connection (and therefore its reservation) stays alive while
    // we test client B. Without this, A's swarm goes unpolled between
    // the end of establish_reservation and the start of the assertion
    // loop, which can let a yamux ping or identify exchange time out and
    // free the quota slot — letting client B succeed and producing a
    // confusing false positive.
    let client_a_driver = tokio::spawn(async move {
        // Loop until cancelled by the test's abort. Panic on stream
        // closure so we surface a real environment break rather than
        // hanging.
        loop {
            let event = client_a
                .next()
                .await
                .expect("client A swarm closed unexpectedly");
            // Don't act on A's events — we only need to keep it polled.
            let _ = event;
        }
    });

    // Client B: tries to reserve, must be denied.
    let mut client_b = Behaviour::build_swarm(&client_config(
        Keypair::generate_ed25519(),
        ephemeral_listen_addr(),
        vec![relay.bootstrap_addr()],
    ))
    .expect("build client B");
    let client_b_local = *client_b.local_peer_id();

    client_b
        .dial(relay.bootstrap_addr())
        .expect("client B dial");

    // Wait for client B's connection to the relay before requesting.
    wait_for(
        &mut client_b,
        "client B ConnectionEstablished",
        Duration::from_secs(10),
        |event| match event {
            SwarmEvent::ConnectionEstablished { peer_id, .. } if *peer_id == relay.peer_id() => {
                Some(())
            }
            _ => None,
        },
    )
    .await;

    let relayed_for_b = relay
        .bootstrap_addr()
        .with(Protocol::P2pCircuit)
        .with(Protocol::P2p(client_b_local));
    let b_listener = client_b
        .listen_on(relayed_for_b)
        .expect("client B listen_on");

    // Client B must observe ListenerClosed (not ExternalAddrConfirmed) for
    // the rejected reservation. The matched ListenerClosed must be for
    // client B's freshly created listener AND must carry an error reason.
    // We don't pattern-match the specific libp2p error variant because the
    // relay-client wraps its protocol errors several Either layers deep
    // and matching against the stringified form would be brittle. Instead
    // we narrow on listener_id + reason.is_err() and corroborate via the
    // relay-side observations.reservations_denied counter below, which
    // gives us a positive signal from the relay's own state machine.
    let result = timeout(Duration::from_secs(15), async {
        loop {
            match client_b.next().await.expect("client B stream") {
                SwarmEvent::ListenerClosed {
                    listener_id,
                    reason,
                    ..
                } if listener_id == b_listener && reason.is_err() => {
                    // Log the actual error for triage when this test does
                    // fail; the wrapped form is too noisy for an assertion
                    // message but useful in the output.
                    eprintln!("client B ListenerClosed reason: {:?}", reason.err());
                    return;
                }
                SwarmEvent::ExternalAddrConfirmed { address } if is_relayed(&address) => {
                    panic!("client B should not have got a reservation; quota was 1");
                }
                _ => {}
            }
        }
    })
    .await;

    client_a_driver.abort();

    assert!(
        result.is_ok(),
        "timed out waiting for ListenerClosed on client B's relayed listener"
    );

    let obs = relay.observations().await;
    assert!(
        obs.reservations_accepted >= 1,
        "client A's reservation must have been accepted; got {obs:?}"
    );
    assert!(
        obs.reservations_denied >= 1,
        "relay must have denied at least one reservation request from client B; got {obs:?}. \
         Without a denial counter increment, the client-side ListenerClosed could be triggered \
         by a different failure mode (e.g. transport teardown) that doesn't exercise the quota \
         path we mean to test."
    );

    relay.shutdown().await;
}
