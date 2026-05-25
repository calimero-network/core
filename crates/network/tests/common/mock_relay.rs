//! In-process libp2p relay server with controllable configuration and
//! fault-injection hooks. Used by integration tests to exercise the
//! reservation lifecycle (request, accept, expire, renew, deny) without
//! depending on a deployed boot-node.
//!
//! The mock runs a real `libp2p::relay::Behaviour` (server side) plus
//! `identify` so a client speaking the production [`calimero_network`]
//! behaviour will discover the hop protocol and request a reservation
//! exactly as it would against the real boot-node.
//!
//! ## Fault injection
//!
//! - [`MockRelay::disconnect_peer`] forcibly closes any active connection to
//!   a client. From the client's perspective, this looks the same as the
//!   relay process crashing or its host going down (the symptom behind the
//!   "node disconnects, can't reconnect without restart" report).
//!
//! - [`MockRelayConfig::max_reservations`] and
//!   [`MockRelayConfig::max_reservations_per_peer`] cap how many concurrent
//!   reservations the server will accept. Used to exercise the "relay quota
//!   exhausted" failure shape — additional clients see `ListenerClosed`
//!   with a `RESOURCE_LIMIT_EXCEEDED`-flavoured reason.
//!
//! - [`MockRelayConfig::reservation_duration`] makes the relay server expire
//!   each reservation after the given duration. Mostly useful for future
//!   tests that need to observe natural expiry; the libp2p relay client
//!   auto-renews aggressively so this alone is not enough to drive a client
//!   into the `ExternalAddrExpired` path — the relay would also need to
//!   actively refuse renewal, which requires more infrastructure than this
//!   mock currently provides.

use core::time::Duration;
use std::sync::{Arc, Mutex as StdMutex};

use eyre::{eyre, Result};
use futures_util::StreamExt;
use libp2p::core::transport::ListenerId;
use libp2p::identity::Keypair;
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::{identify, noise, relay, tcp, yamux, Multiaddr, PeerId, Swarm, SwarmBuilder};
use multiaddr::Protocol;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::timeout;
use tracing::{debug, warn};

const BIND_TIMEOUT: Duration = Duration::from_secs(10);

/// Identify protocol version advertised by the mock. Distinct from the real
/// boot-node string so logs make the source obvious.
const MOCK_IDENTIFY_PROTOCOL_VERSION: &str = "/calimero/mock-relay/1.0.0";
const MOCK_IDENTIFY_AGENT_VERSION: &str = "calimero-mock-relay/test";

#[derive(NetworkBehaviour)]
struct MockBehaviour {
    relay: relay::Behaviour,
    identify: identify::Behaviour,
}

/// Tunable parameters for the mock. Defaults track the libp2p-relay 0.21
/// defaults.
#[derive(Clone, Debug)]
pub struct MockRelayConfig {
    pub max_reservations: usize,
    pub max_reservations_per_peer: usize,
    /// How long each reservation lasts before the server expires it.
    /// Production boot-node uses 1 hour; tests typically want hundreds of
    /// milliseconds so they don't have to wait.
    pub reservation_duration: Duration,
    pub max_circuits: usize,
    pub max_circuits_per_peer: usize,
    pub max_circuit_duration: Duration,
    pub max_circuit_bytes: u64,
}

impl Default for MockRelayConfig {
    fn default() -> Self {
        Self {
            max_reservations: 128,
            max_reservations_per_peer: 4,
            reservation_duration: Duration::from_secs(3600),
            max_circuits: 1024,
            max_circuits_per_peer: 16,
            max_circuit_duration: Duration::from_secs(3600),
            max_circuit_bytes: 1 << 30,
        }
    }
}

impl MockRelayConfig {
    fn into_relay_config(self) -> relay::Config {
        relay::Config {
            max_reservations: self.max_reservations,
            max_reservations_per_peer: self.max_reservations_per_peer,
            reservation_duration: self.reservation_duration,
            reservation_rate_limiters: vec![],
            max_circuits: self.max_circuits,
            max_circuits_per_peer: self.max_circuits_per_peer,
            max_circuit_duration: self.max_circuit_duration,
            max_circuit_bytes: self.max_circuit_bytes,
            circuit_src_rate_limiters: vec![],
        }
    }
}

/// Snapshot of events the mock observed. Useful for assertions about what
/// the relay actually saw the client do (independent of what the client
/// thinks happened).
#[derive(Clone, Debug, Default)]
pub struct MockRelayObservations {
    pub reservations_accepted: usize,
    pub reservations_denied: usize,
    pub circuits_opened: usize,
}

/// Commands the mock task accepts on its control channel.
enum Command {
    DisconnectPeer {
        peer: PeerId,
        ack: oneshot::Sender<bool>,
    },
    Shutdown {
        ack: oneshot::Sender<()>,
    },
}

/// Handle to a running mock relay server. Drop or call [`Self::shutdown`]
/// to stop it.
///
/// `abort` is kept separately from the join handle (without a lock) so
/// [`Drop`] can always cancel the background task even if [`Self::shutdown`]
/// is concurrently holding the join-handle mutex.
///
/// `observations` is a [`std::sync::Mutex`] shared with the swarm task. The
/// critical section is just an increment-or-clone, never held across an
/// await, so the synchronous mutex is correct here and avoids the lock-
/// across-await footgun an async `tokio::sync::Mutex` would introduce
/// inside the swarm's `select!`. It also means `observations()` is
/// infallible — it reads shared state directly rather than racing a
/// command roundtrip against task shutdown.
pub struct MockRelay {
    peer_id: PeerId,
    listen_addr: Multiaddr,
    cmd_tx: mpsc::Sender<Command>,
    abort: AbortHandle,
    join: Mutex<Option<JoinHandle<()>>>,
    observations: Arc<StdMutex<MockRelayObservations>>,
}

impl MockRelay {
    /// Spawn a mock relay with default config, listening on a random local
    /// TCP port. Returns once the listener has reported a concrete address.
    pub async fn spawn() -> Result<Self> {
        Self::spawn_with(MockRelayConfig::default(), Keypair::generate_ed25519()).await
    }

    /// Spawn with a custom config and identity. Tests that need to "restart"
    /// the relay should keep the keypair around and reuse it.
    pub async fn spawn_with(config: MockRelayConfig, keypair: Keypair) -> Result<Self> {
        let peer_id = PeerId::from(keypair.public());
        let relay_config = config.into_relay_config();

        let mut swarm = SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )?
            .with_behaviour(|key| MockBehaviour {
                relay: relay::Behaviour::new(peer_id, relay_config),
                identify: identify::Behaviour::new(
                    identify::Config::new(MOCK_IDENTIFY_PROTOCOL_VERSION.to_owned(), key.public())
                        .with_agent_version(MOCK_IDENTIFY_AGENT_VERSION.to_owned()),
                ),
            })
            .map_err(|err| eyre!("failed to build mock relay behaviour: {err}"))?
            .build();

        let _listener: ListenerId = swarm.listen_on("/ip4/127.0.0.1/tcp/0".parse()?)?;

        // Wait for the listener to report a concrete bound address. If the
        // listener fails to bind (e.g. on a constrained CI environment) the
        // swarm emits ListenerError / ListenerClosed before NewListenAddr;
        // surface those explicitly instead of spinning until the stream
        // closes with a generic message. A hard timeout caps the wait so a
        // wedged kernel can't hang the entire test runner.
        let listen_addr = timeout(BIND_TIMEOUT, async {
            loop {
                match swarm.next().await {
                    Some(SwarmEvent::NewListenAddr { address, .. }) => return Ok(address),
                    Some(SwarmEvent::ListenerError { error, .. }) => {
                        return Err(eyre!("mock relay listener error during bind: {error}"));
                    }
                    Some(SwarmEvent::ListenerClosed {
                        reason: Err(error), ..
                    }) => {
                        return Err(eyre!("mock relay listener closed during bind: {error}"));
                    }
                    Some(_) => continue,
                    None => return Err(eyre!("mock relay swarm closed before listening")),
                }
            }
        })
        .await
        .map_err(|_| {
            eyre!(
                "mock relay did not bind within {:?} — likely a kernel or libp2p hang",
                BIND_TIMEOUT
            )
        })??;

        // libp2p's relay::Behaviour fills the reservation response with the
        // SERVER's external addresses, not the client's listen addresses, so
        // the client can build a `<relay>/p2p-circuit/<self>` multiaddr.
        // For a server bound only to a loopback TCP port, swarm.external_addresses()
        // is empty by default — AutoNAT would normally populate it, but we
        // don't run AutoNAT here. Tell the relay explicitly that its bound
        // address is its external address. Without this, every reservation
        // attempt returns NoAddressesInReservation and the client's listener
        // is torn down before it can be used.
        swarm.add_external_address(listen_addr.clone());

        let (cmd_tx, mut cmd_rx) = mpsc::channel::<Command>(16);
        let observations = Arc::new(StdMutex::new(MockRelayObservations::default()));

        let observations_for_task = Arc::clone(&observations);
        let join = tokio::spawn(async move {
            run_swarm(swarm, &mut cmd_rx, observations_for_task).await;
        });
        let abort = join.abort_handle();

        Ok(Self {
            peer_id,
            listen_addr,
            cmd_tx,
            abort,
            join: Mutex::new(Some(join)),
            observations,
        })
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// The relay's address in the form a client would put in its bootstrap
    /// config: `/ip4/.../tcp/<port>/p2p/<peer_id>`.
    pub fn bootstrap_addr(&self) -> Multiaddr {
        self.listen_addr.clone().with(Protocol::P2p(self.peer_id))
    }

    /// Forcibly close any active connections to `peer`. Returns true if at
    /// least one connection was found.
    ///
    /// Panics if the relay task is no longer running — a test that calls
    /// this expects the relay to be alive, and an Err on the command
    /// channel or a dropped ack would otherwise be conflated with the
    /// semantic "peer not connected" return, producing misleading
    /// assertion messages at call sites.
    pub async fn disconnect_peer(&self, peer: PeerId) -> bool {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.cmd_tx
            .send(Command::DisconnectPeer { peer, ack: ack_tx })
            .await
            .expect("mock relay task is not running (cmd channel closed)");
        ack_rx
            .await
            .expect("mock relay task dropped the disconnect ack (likely panicked)")
    }

    /// Snapshot of what the relay has observed since spawn. Reads shared
    /// state directly, so it's infallible — there is no task-roundtrip that
    /// could race with shutdown.
    pub fn observations(&self) -> MockRelayObservations {
        self.observations
            .lock()
            .expect("observations mutex poisoned")
            .clone()
    }

    /// Stop the relay. Idempotent; subsequent calls are no-ops.
    ///
    /// The cmd-channel send and ack receive are best-effort — if the task
    /// has already exited cleanly they fail silently, which is fine. But
    /// the final `handle.await` is the authoritative liveness check: a
    /// panic inside the swarm task surfaces as `JoinError::is_panic()`
    /// here and we propagate it via `panic!` so the test fails loudly
    /// rather than letting the panic disappear into stderr.
    pub async fn shutdown(&self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(Command::Shutdown { ack: ack_tx }).await;
        let _ = ack_rx.await;

        let mut guard = self.join.lock().await;
        if let Some(handle) = guard.take() {
            match handle.await {
                Ok(()) => {}
                Err(err) if err.is_cancelled() => {
                    // Drop fired and aborted the task before we got here.
                    // Not a panic — accept silently.
                }
                Err(err) => {
                    panic!("mock relay task panicked: {err:?}");
                }
            }
        }
    }
}

impl Drop for MockRelay {
    fn drop(&mut self) {
        // Abort unconditionally. AbortHandle::abort is a no-op if the task
        // has already finished (e.g. shutdown() already ran), so this is
        // safe to call whether or not the task is still active. We
        // deliberately do not lock self.join here — if shutdown() is
        // concurrently holding it, we must still guarantee the task is
        // cancelled.
        //
        // Best-effort by design: Drop cannot await the JoinHandle, so a
        // panic inside the swarm task that happens *after* abort is silently
        // lost. shutdown() is the authoritative stop path and propagates
        // task panics; Drop is the fallback for unjoined leaks.
        self.abort.abort();
    }
}

async fn run_swarm(
    mut swarm: Swarm<MockBehaviour>,
    cmd_rx: &mut mpsc::Receiver<Command>,
    observations: Arc<StdMutex<MockRelayObservations>>,
) {
    loop {
        // `biased;` gives control commands priority over swarm events. Without
        // it, under high-frequency swarm traffic (e.g. many incoming
        // connections during a quota test) the cmd branch could be starved,
        // delaying Shutdown long enough that callers time out.
        tokio::select! {
            biased;
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(Command::DisconnectPeer { peer, ack }) => {
                        let result = swarm.disconnect_peer_id(peer).is_ok();
                        let _ = ack.send(result);
                    }
                    Some(Command::Shutdown { ack }) => {
                        let _ = ack.send(());
                        break;
                    }
                    None => break,
                }
            }
            event = swarm.next() => {
                let Some(event) = event else { break; };
                match event {
                    SwarmEvent::Behaviour(MockBehaviourEvent::Relay(relay_event)) => {
                        debug!(?relay_event, "mock relay: relay event");
                        // Synchronous std::sync::Mutex — the critical section
                        // is a single integer increment, never held across an
                        // await. No async-lock-in-select! footgun.
                        let mut obs =
                            observations.lock().expect("observations mutex poisoned");
                        match relay_event {
                            relay::Event::ReservationReqAccepted { .. } => {
                                obs.reservations_accepted += 1;
                            }
                            relay::Event::ReservationReqDenied { .. } => {
                                obs.reservations_denied += 1;
                            }
                            relay::Event::CircuitReqAccepted { .. } => {
                                obs.circuits_opened += 1;
                            }
                            _ => {}
                        }
                    }
                    SwarmEvent::Behaviour(MockBehaviourEvent::Identify(_)) => {}
                    SwarmEvent::IncomingConnectionError { error, .. } => {
                        warn!(%error, "mock relay: incoming connection error");
                    }
                    _ => {}
                }
            }
        }
    }
    debug!("mock relay shutting down");
}
