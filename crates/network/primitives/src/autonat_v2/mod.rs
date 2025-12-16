//! AutoNAT v2 behaviour module.
//!
//! This module provides a switchable AutoNAT v2 behaviour that can operate in
//! client-only or client-and-server modes. It wraps the libp2p autonat v2
//! client and server behaviours with intelligent handler routing.
//!
//! Key features:
//! - Track which peers have server support
//! - Closes connections when switching modes to ensure clean handler state
//! - Uses protocol negotiation detection to make intelligent routing decisions

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

use libp2p::autonat::v2::{client, server};
use libp2p::swarm::ConnectionId;
use libp2p::{Multiaddr, PeerId};
use rand::rngs::OsRng;

mod behaviour;

/// A NetworkBehaviour that can switch between AutoNAT v2 client and server modes.
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

/// Operating mode for the AutoNAT v2 behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Only acting as a client
    ClientOnly,

    /// Acting as both client and server
    ClientAndServer,
}

/// Events emitted by the AutoNAT v2 behaviour.
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
        /// The result of the test.
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
    ModeChanged {
        /// The previous mode.
        old_mode: Mode,
        /// The new mode.
        new_mode: Mode,
    },

    /// Discovered a peer has server support
    PeerHasServerSupport {
        /// The peer ID of the peer with server support.
        peer_id: PeerId,
    },
}

/// Result of an address test.
#[derive(Debug, Clone)]
pub enum TestResult {
    /// Address is reachable
    Reachable {
        /// The address that was confirmed as reachable
        addr: Multiaddr,
    },

    /// Test failed
    Failed {
        /// Description of the failure
        error: String,
    },
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

/// Configuration for the AutoNAT v2 behaviour.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// How many untested address candidates to keep track of
    pub max_candidates: usize,

    /// The interval at which the probe will attempt to confirm candidates
    /// as external addresses
    pub probe_interval: Duration,
}

impl Config {
    /// Set the maximum number of address candidates to track.
    pub fn with_max_candidates(self, max_candidates: usize) -> Self {
        Self {
            max_candidates,
            ..self
        }
    }

    /// Set the probe interval for testing address candidates.
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
    /// Create a new switchable AutoNAT v2 behaviour starting in client-only mode.
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

    /// Get the current mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Check if server is enabled.
    pub fn is_server_enabled(&self) -> bool {
        self.server.is_some()
    }

    /// Check if a peer has server support.
    pub fn peer_has_server_support(&self, peer_id: &PeerId) -> bool {
        self.peers_with_server_support.contains(peer_id)
    }

    /// Get all peers with known server support.
    pub fn peers_with_server_support(&self) -> impl Iterator<Item = &PeerId> {
        self.peers_with_server_support.iter()
    }

    /// Enable server mode.
    ///
    /// # Errors
    ///
    /// Returns an error if server mode is already enabled.
    pub fn enable_server(&mut self) -> Result<(), String> {
        if self.server.is_some() {
            return Err("Server already enabled".to_owned());
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

    /// Disable server mode and close all connections using server handlers.
    ///
    /// # Errors
    ///
    /// Returns an error if server mode is already disabled.
    pub fn disable_server(&mut self) -> Result<(), String> {
        if self.server.is_none() {
            return Err("Server already disabled".to_owned());
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
