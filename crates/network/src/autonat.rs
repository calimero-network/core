use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use libp2p::autonat::v2::{client, server};
use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId};
use rand::rngs::OsRng;

mod behaviour;

/// A NetworkBehaviour that can switch between AutoNAT v2 client and server modes.
///
/// Key features:
/// - Track which peers have server support
/// - Closes connections when switching modes to ensure clean handler state
/// - Uses protocol negotiation detection to make intelligent routing decisions
#[expect(
    missing_debug_implementations,
    reason = "Swarm behaviours don't implement Debug"
)]
pub struct Behaviour {
    /// Current operation mode
    mode: Mode,

    /// Client behavior (always present)
    client: client::Behaviour,

    /// Server behavior (optional)
    server: Option<server::Behaviour>,

    /// Queued events to emit
    events: VecDeque<Event>,

    /// Track which peers support AutoNAT server behavior
    peers_with_server_support: HashSet<PeerId>,

    /// Track connections and their roles
    connection_info: HashMap<ConnectionId, ConnectionInfo>,

    /// Connections to close on next poll
    connections_to_close: Vec<(PeerId, ConnectionId)>,

    /// Track peers we're dialing back to (server dial-backs)
    server_dialback_peers: HashSet<PeerId>,

    /// Track peers we're expecting dial-backs from (client requested tests)
    client_expecting_dialback: HashSet<PeerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only acting as a client
    ClientOnly,

    /// Acting as both client and server
    ClientAndServer,
}

#[derive(Debug, Clone)]
pub enum Event {
    /// Client tested an address through a server
    Client {
        /// The address that was selected for testing.
        tested_addr: Multiaddr,
        /// The amount of data that was sent to the server.
        /// Is 0 if it wasn't necessary to send any data.
        /// Otherwise it's a number between 30.000 and 100.000.
        bytes_sent: usize,
        /// The peer id of the server that was selected for testing.
        server: PeerId,
        result: TestResult,
    },

    /// Event from the server behaviour
    Server {
        /// All address that were submitted for testing.
        all_addrs: Vec<Multiaddr>,
        /// The address that was eventually tested.
        tested_addr: Multiaddr,
        /// The peer id of the client that submitted addresses for testing.
        client: PeerId,
        /// The amount of data that was requested by the server and was transmitted.
        data_amount: usize,
        /// The result of the test.
        result: TestResult,
    },

    /// Mode has changed
    ModeChanged { old_mode: Mode, new_mode: Mode },

    /// Discovered a peer has server support
    PeerHasServerSupport { peer_id: PeerId },
}

/// Result of an address test
#[derive(Debug, Clone)]
pub enum TestResult {
    /// Address is reachable
    Reachable {
        /// The address that was confirmed as reachable
        addr: Multiaddr,
    },

    /// Test failed
    Failed { error: String },
}

#[derive(Debug, Clone)]
struct ConnectionInfo {
    peer_id: PeerId,
    role: ConnectionRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionRole {
    /// Connection using client handler
    Client,

    /// Connection using server handler
    Server,
}

#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// How many untested address candidates to keep track of
    pub max_candidates: usize,

    /// The interval at which the probe will attempt to confirm candidates
    /// as external addresses
    pub probe_interval: Duration,
}

impl Config {
    pub fn with_max_candidates(self, max_candidates: usize) -> Self {
        Self {
            max_candidates,
            ..self
        }
    }

    pub fn with_probe_interval(self, probe_interval: Duration) -> Self {
        Self {
            probe_interval,
            ..self
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_candidates: 10,
            probe_interval: Duration::from_secs(5),
        }
    }
}

impl Behaviour {
    /// Create a new switchable AutoNAT v2 behaviour starting in client-only mode
    pub fn new(cfg: Config) -> Self {
        let client = client::Behaviour::new(
            OsRng,
            client::Config::default()
                .with_max_candidates(cfg.max_candidates)
                .with_probe_interval(cfg.probe_interval),
        );

        Self {
            mode: Mode::ClientOnly,
            client,
            server: None,
            events: VecDeque::new(),
            peers_with_server_support: HashSet::new(),
            connection_info: HashMap::new(),
            connections_to_close: Vec::new(),
            server_dialback_peers: HashSet::new(),
            client_expecting_dialback: HashSet::new(),
        }
    }

    /// Get the current mode
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Check if server is enabled
    pub fn is_server_enabled(&self) -> bool {
        self.server.is_some()
    }

    /// Check if a peer has server support
    pub fn peer_has_server_support(&self, peer_id: &PeerId) -> bool {
        self.peers_with_server_support.contains(peer_id)
    }

    /// Get all peers with known server support
    pub fn peers_with_server_support(&self) -> impl Iterator<Item = &PeerId> {
        self.peers_with_server_support.iter()
    }

    /// Enable server mode
    pub fn enable_server(&mut self) -> Result<(), String> {
        if self.server.is_some() {
            return Err("Server already enabled".to_string());
        }

        let server = server::Behaviour::default();
        self.server = Some(server);

        let old_mode = self.mode;
        self.mode = Mode::ClientAndServer;

        // No need to clear client_expecting_dialback here
        // The client is still active and expectations are still valid
        self.events.push_back(Event::ModeChanged {
            old_mode,
            new_mode: self.mode,
        });

        Ok(())
    }

    /// Disable server mode and close all connections using server handlers
    pub fn disable_server(&mut self) -> Result<(), String> {
        if self.server.is_none() {
            return Err("Server already disabled".to_string());
        }

        let old_mode = self.mode;
        self.server = None;
        self.mode = Mode::ClientOnly;

        // Clear server-related state only
        // Note: Do NOT clear client_expecting_dialback here - the client is still
        // active and may have in-flight NAT test requests expecting dial-backs
        self.server_dialback_peers.clear();

        // Close all connections that are using server handlers
        let server_connections: Vec<_> = self
            .connection_info
            .iter()
            .filter(|(_, info)| info.role == ConnectionRole::Server)
            .map(|(conn_id, info)| (info.peer_id, *conn_id))
            .collect();

        for (peer_id, conn_id) in server_connections {
            tracing::debug!(
                %peer_id,
                %conn_id,
                "Closing connection with server handler due to mode switch"
            );
            self.connections_to_close.push((peer_id, conn_id));
        }

        self.events.push_back(Event::ModeChanged {
            old_mode,
            new_mode: self.mode,
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
    use libp2p::{identify, Swarm};
    use libp2p_swarm_test::{drive, SwarmExt};

    use super::*;

    #[derive(NetworkBehaviour)]
    struct Client {
        autonat: client::Behaviour,
        identify: identify::Behaviour,
    }

    #[derive(NetworkBehaviour)]
    struct Server {
        autonat: server::Behaviour,
        identify: identify::Behaviour,
    }

    #[derive(NetworkBehaviour)]
    struct SwitchableNat {
        autonat: Behaviour,
        identify: identify::Behaviour,
    }

    async fn new_server() -> Swarm<Server> {
        let mut node = Swarm::new_ephemeral_tokio(|identity| Server {
            autonat: server::Behaviour::default(),
            identify: identify::Behaviour::new(identify::Config::new(
                "/libp2p-test/1.0.0".into(),
                identity.public().clone(),
            )),
        });
        let _res = node.listen().with_tcp_addr_external().await;

        node
    }

    async fn new_client() -> Swarm<Client> {
        let mut node = Swarm::new_ephemeral_tokio(|identity| Client {
            autonat: client::Behaviour::new(
                OsRng,
                client::Config::default().with_probe_interval(Duration::from_millis(100)),
            ),
            identify: identify::Behaviour::new(identify::Config::new(
                "/libp2p-test/1.0.0".into(),
                identity.public().clone(),
            )),
        });
        let _res = node.listen().await;
        node
    }

    // NOTE: Without identify, the client dial_request handler never learns
    // the server supports AutoNAT, so it never sends probes!
    //
    // The identify handler calls `ConnectionHandlerEvent::ReportRemoteProtocols`, which gets
    // translated by the swarm into `ConnectionEvent::RemoteProtocolsChange` and delivered to all
    // other handlers on that connection!
    //
    // The client dial_request handler checks this event and emits a `ToBehaviour::PeerHasServerSupport` event.
    // Which is then used by client behaviour to chose random autonat servers for probes.
    fn new_switchable_no_listener() -> Swarm<SwitchableNat> {
        let node = Swarm::new_ephemeral_tokio(|identity| {
            let cfg = Config::default().with_probe_interval(Duration::from_millis(100));
            SwitchableNat {
                autonat: Behaviour::new(cfg),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/libp2p-test/1.0.0".into(),
                    identity.public().clone(),
                )),
            }
        });

        node
    }

    #[tokio::test]
    async fn test_initial_mode_client_only() {
        let swarm = new_switchable_no_listener();

        assert_eq!(swarm.behaviour().autonat.mode(), Mode::ClientOnly);
        assert!(!swarm.behaviour().autonat.is_server_enabled())
    }

    #[tokio::test]
    async fn test_mode_changes() {
        let mut swarm = new_switchable_no_listener();

        // Enable AutoNAT server
        swarm.behaviour_mut().autonat.enable_server().unwrap();

        assert_eq!(swarm.behaviour().autonat.mode(), Mode::ClientAndServer);
        assert!(swarm.behaviour().autonat.is_server_enabled());

        // Check we got mode changed event
        let (old_mode, new_mode) = swarm
            .wait(|e| match e {
                SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(Event::ModeChanged {
                    old_mode,
                    new_mode,
                })) => Some((old_mode, new_mode)),
                _ => None,
            })
            .await;

        assert_eq!(old_mode, Mode::ClientOnly);
        assert_eq!(new_mode, Mode::ClientAndServer);

        // Disable AutoNAT server
        swarm.behaviour_mut().autonat.disable_server().unwrap();

        assert_eq!(swarm.behaviour().autonat.mode(), Mode::ClientOnly);
        assert!(!swarm.behaviour().autonat.is_server_enabled());

        // Check we got mode changed event, again
        let (old_mode, new_mode) = swarm
            .wait(|e| match e {
                SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(Event::ModeChanged {
                    old_mode,
                    new_mode,
                })) => Some((old_mode, new_mode)),
                _ => None,
            })
            .await;

        assert_eq!(old_mode, Mode::ClientAndServer);
        assert_eq!(new_mode, Mode::ClientOnly);
    }

    #[tokio::test]
    async fn test_double_enable_disable_fails() {
        let mut swarm = new_switchable_no_listener();

        // Enable AutoNAT server two times
        swarm.behaviour_mut().autonat.enable_server().unwrap();
        let result = swarm.behaviour_mut().autonat.enable_server();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Server already enabled");

        // Disable AutoNAT server two times
        swarm.behaviour_mut().autonat.disable_server().unwrap();
        let result = swarm.behaviour_mut().autonat.disable_server();

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "Server already disabled");
    }

    #[tokio::test]
    async fn test_switchable_client_and_server_connection() {
        let mut server = new_server().await;
        let mut switchable = new_switchable_no_listener();
        let _addr = switchable.listen().await;

        switchable.connect(&mut server).await;

        // Connection is already established, just verify peer IDs
        assert!(switchable.is_connected(server.local_peer_id()));
        assert!(server.is_connected(switchable.local_peer_id()));
    }

    #[tokio::test]
    async fn test_dynamic_protocol_change() {
        let mut server = new_server().await;
        let server_id = *server.local_peer_id();
        let mut switchable = new_switchable_no_listener();
        let _addr = switchable.listen().await;

        switchable.connect(&mut server).await;

        // Test initial state: switchable is client-only (regular client server interaction)
        match drive(&mut switchable, &mut server).await {
            (
                [SwitchableNatEvent::Identify(_), SwitchableNatEvent::Identify(identify::Event::Received {
                    info: first_srv_info,
                    ..
                }), SwitchableNatEvent::Autonat(Event::PeerHasServerSupport { .. }), SwitchableNatEvent::Identify(_), SwitchableNatEvent::Identify(identify::Event::Received {
                    info: second_srv_info,
                    ..
                })],
                [ServerEvent::Identify(_), ServerEvent::Identify(identify::Event::Received {
                    info: first_switchable_info,
                    ..
                }), ServerEvent::Identify(_), ServerEvent::Identify(identify::Event::Received {
                    info: second_switchable_info,
                    ..
                })],
            ) => {
                // First exchange: Switchable sees server has dial-request
                assert!(
                    first_srv_info
                        .protocols
                        .iter()
                        .any(|p| p.as_ref() == "/libp2p/autonat/2/dial-request"),
                    "First exchange: Server should advertise dial-request, got: {:?}",
                    first_srv_info.protocols
                );

                // First exchange: Server sees switchable has no autonat protocols yet
                assert!(
                    !first_switchable_info
                        .protocols
                        .iter()
                        .any(|p| p.as_ref().contains("/libp2p/autonat/2/")),
                    "First exchange: Switchable shouldn't have autonat protocols yet, got: {:?}",
                    first_switchable_info.protocols
                );

                // Second exchange: Switchable sees server has no autonat protocols
                assert!(
                    !second_srv_info
                        .protocols
                        .iter()
                        .any(|p| p.as_ref().contains("/libp2p/autonat/2/")),
                    "Second exchange: Server shouldn't advertise autonat protocols, got: {:?}",
                    second_srv_info.protocols
                );

                // Second exchange: Server sees switchable now has dial-back
                assert!(
                    second_switchable_info
                        .protocols
                        .iter()
                        .any(|p| p.as_ref() == "/libp2p/autonat/2/dial-back"),
                    "Second exchange: Switchable should advertise dial-back, got: {:?}",
                    second_switchable_info.protocols
                );
            }
            other => panic!("Unexpected events: {other:?}"),
        }

        // Wait for server to complete the AutoNAT test
        let confirmed_addr = match drive(&mut switchable, &mut server).await {
            (
                [SwarmEvent::ExternalAddrConfirmed { address }, SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(_))],
                [ServerEvent::Autonat(event)],
            ) => {
                assert!(matches!(event.result, Ok(())));
                address
            }
            other => panic!("Unexpected events: {other:?}"),
        };

        // Now switch to server mode
        switchable.behaviour_mut().autonat.enable_server().unwrap();
        // Wait for mode change
        let new_mode = switchable
            .wait(|e| match e {
                SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(Event::ModeChanged {
                    new_mode,
                    ..
                })) => Some(new_mode),
                _ => None,
            })
            .await;
        assert!(matches!(new_mode, Mode::ClientAndServer));

        // Remove previously confirmed address
        switchable.remove_external_address(&confirmed_addr);
        assert!(
            !switchable
                .external_addresses()
                .any(|a| a == &confirmed_addr),
            "External address should have been removed"
        );

        // Disconnect from the server
        switchable.disconnect_peer_id(server_id).unwrap();
        // Wait for the disconnect to complete
        match drive(&mut switchable, &mut server).await {
            (
                [SwarmEvent::ConnectionClosed { .. }, SwarmEvent::ConnectionClosed { .. }],
                [SwarmEvent::ConnectionClosed { .. }, SwarmEvent::ConnectionClosed { .. }],
            ) => {}
            other => panic!("Unexpected events: {other:?}"),
        }

        // NOTE: there's a bug in the code that causes the autonat behaviour to not work correctly
        // AutoNAT Client behaviour never clears out the already confirmed address,
        // event though those have been expired
        let cfg = Config::default().with_probe_interval(Duration::from_millis(100)); // <- until resolved; manually set new behaviour
        switchable.behaviour_mut().autonat = Behaviour::new(cfg);
        switchable.behaviour_mut().autonat.enable_server().unwrap();

        // Conect againg with the server
        // let _addr = switchable.listen().await;
        let _addr = server.listen().with_tcp_addr_external().await;
        switchable.connect(&mut server).await;

        // Drive the server swarm events
        let _handle = tokio::spawn(server.loop_on_next());

        switchable
            .wait(|e| match e {
                SwarmEvent::ExternalAddrConfirmed { .. } => Some(()),
                _ => None,
            })
            .await;

        // Check if the switchable knows the server has support
        assert!(switchable
            .behaviour()
            .autonat
            .peer_has_server_support(&server_id));
    }

    #[tokio::test]
    async fn test_server_support_tracking() {
        let mut server = new_server().await;
        let mut switchable = new_switchable_no_listener();
        let _addr = switchable.listen().await;

        switchable.connect(&mut server).await;

        // Drive server swarm events
        let server_id = *server.local_peer_id();
        let _handle = tokio::spawn(server.loop_on_next());

        // Wait for server to be tracked and confirmed
        let tracked_server_id = switchable
            .wait(|e| match e {
                SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(
                    Event::PeerHasServerSupport { peer_id },
                )) => Some(peer_id),
                _ => None,
            })
            .await;

        assert_eq!(tracked_server_id, server_id);
    }

    #[tokio::test]
    async fn test_server_disable_closes_connections() {
        let mut client = new_client().await;
        let mut switchable = new_switchable_no_listener();
        let _addr = switchable.listen().with_tcp_addr_external().await;

        // Enable server mode on switchable swarm
        switchable.behaviour_mut().autonat.enable_server().unwrap();

        client.connect(&mut switchable).await;

        // Verify connection is established
        assert!(switchable.is_connected(client.local_peer_id()));

        // Disable server - this should close connections with client
        switchable.behaviour_mut().autonat.disable_server().unwrap();

        // Wait for mode change
        switchable
            .wait(|e| match e {
                SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(Event::ModeChanged {
                    ..
                })) => Some(()),
                _ => None,
            })
            .await;

        // Wait for connection close
        let disconnected_client_id = switchable
            .wait(|e| match e {
                SwarmEvent::ConnectionClosed { peer_id, .. } => Some(peer_id),
                _ => None,
            })
            .await;

        // Check if still connected; should be false
        assert_eq!(disconnected_client_id, *client.local_peer_id());
        assert!(!switchable.is_connected(client.local_peer_id()));
    }

    #[tokio::test]
    async fn test_handler_selection_outbound_to_known_server() {
        let mut client = new_switchable_no_listener();
        let mut server = new_switchable_no_listener();

        // Start listening
        let _addr = client.listen().await;
        let _addr = server.listen().with_tcp_addr_external().await;
        // Enable server mode on both (client can also act as server)
        client.behaviour_mut().autonat.enable_server().unwrap();
        server.behaviour_mut().autonat.enable_server().unwrap();

        // First connection - client doesn't know server has support yet
        client.connect(&mut server).await;
        // Drive server swarm events
        let server_id = *server.local_peer_id();
        let _handle = tokio::spawn(server.loop_on_next());

        // Wait for server support discovery
        let learned_server_id = client
            .wait(|e| match e {
                SwarmEvent::Behaviour(SwitchableNatEvent::Autonat(
                    Event::PeerHasServerSupport { peer_id },
                )) => Some(peer_id),
                _ => None,
            })
            .await;

        // Test if event passed propper ID from discovered server
        assert_eq!(learned_server_id, server_id);

        // Check the server's actually doing what it says it's doing
        client
            .wait(|e| match e {
                SwarmEvent::ExternalAddrConfirmed { .. } => Some(()),
                _ => None,
            })
            .await;

        // Check if the client knows the server has support
        assert!(client
            .behaviour()
            .autonat
            .peer_has_server_support(&server_id));

        // The client's connection info should show it used the appropriate handler
        // (This is internal state, but we can verify behavior indirectly)
        assert!(client.is_connected(&server_id));
    }
}
