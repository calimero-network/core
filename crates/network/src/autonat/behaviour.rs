use std::task::{Context, Poll};

use either::Either;
use libp2p::autonat::v2::{client, server};
use libp2p::core::transport::PortUse;
use libp2p::core::Endpoint;
use libp2p::swarm::{
    CloseConnection, ConnectionDenied, ConnectionHandler, ConnectionId, FromSwarm,
    NetworkBehaviour, THandler, THandlerInEvent, THandlerOutEvent, ToSwarm,
};
use libp2p::{Multiaddr, PeerId};

use crate::autonat::{Behaviour, ConnectionInfo, ConnectionRole, Event, TestResult};

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = Either<
        <client::Behaviour as NetworkBehaviour>::ConnectionHandler,
        <server::Behaviour as NetworkBehaviour>::ConnectionHandler,
    >;

    type ToSwarm = Event;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        self.client
            .handle_pending_inbound_connection(connection_id, local_addr, remote_addr)?;

        if let Some(server) = &mut self.server {
            server.handle_pending_inbound_connection(connection_id, local_addr, remote_addr)?;
        }

        Ok(())
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        // For inbound connections:
        // 1. If this is a dial-back our server is making -> use server handler
        // 2. If we have server enabled and this is NOT a dial-back -> use server handler
        //    (the peer is connecting to us to potentially request a NAT test)
        // 3. Otherwise (client-only mode) -> use client handler
        //    (we might receive a dial-back from a server we requested a test from)

        let is_server_dialback = self.server_dialback_peers.contains(&peer);

        if is_server_dialback {
            // This connection is part of a server dial-back we're doing
            if let Some(server) = &mut self.server {
                let handler = server.handle_established_inbound_connection(
                    connection_id,
                    peer,
                    local_addr,
                    remote_addr,
                )?;

                _ = self.connection_info.insert(
                    connection_id,
                    ConnectionInfo {
                        peer_id: peer,
                        role: ConnectionRole::Server,
                    },
                );

                tracing::debug!(%peer, %connection_id, "Inbound: server dial-back");
                return Ok(Either::Right(handler));
            }
        }

        // Check if we expect a dial-back from this peer (we requested a test as client)
        let expecting_client_dialback = self.client_expecting_dialback.remove(&peer);

        if expecting_client_dialback {
            // We're expecting a dial-back because we requested a NAT test
            let handler = self.client.handle_established_inbound_connection(
                connection_id,
                peer,
                local_addr,
                remote_addr,
            )?;

            _ = self.connection_info.insert(
                connection_id,
                ConnectionInfo {
                    peer_id: peer,
                    role: ConnectionRole::Client,
                },
            );

            tracing::debug!(%peer, %connection_id, "Inbound: client expecting dial-back");
            return Ok(Either::Left(handler));
        }

        // Default behavior based on mode
        if let Some(server) = &mut self.server {
            // Server mode: accept incoming requests
            let handler = server.handle_established_inbound_connection(
                connection_id,
                peer,
                local_addr,
                remote_addr,
            )?;

            _ = self.connection_info.insert(
                connection_id,
                ConnectionInfo {
                    peer_id: peer,
                    role: ConnectionRole::Server,
                },
            );

            tracing::debug!(%peer, %connection_id, "Inbound: server mode (accepting requests)");
            Ok(Either::Right(handler))
        } else {
            // Client-only mode
            let handler = self.client.handle_established_inbound_connection(
                connection_id,
                peer,
                local_addr,
                remote_addr,
            )?;

            _ = self.connection_info.insert(
                connection_id,
                ConnectionInfo {
                    peer_id: peer,
                    role: ConnectionRole::Client,
                },
            );

            tracing::debug!(%peer, %connection_id, "Inbound: client-only mode");
            Ok(Either::Left(handler))
        }
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        // Check if server wants to dial back first
        if let Some(server) = &mut self.server {
            let server_addrs = server.handle_pending_outbound_connection(
                connection_id,
                maybe_peer,
                addresses,
                effective_role,
            )?;

            // If server returned addresses, it's initiating a dial-back
            if !server_addrs.is_empty() {
                if let Some(peer) = maybe_peer {
                    _ = self.server_dialback_peers.insert(peer);
                    tracing::debug!(%peer, "Server initiating dial-back to client");
                }
                return Ok(server_addrs);
            }
        }

        // Otherwise let client handle
        let client_addrs = self.client.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;

        Ok(client_addrs)
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        // Check if this is a server dial-back we initiated
        let is_server_dialback = self.server_dialback_peers.remove(&peer);

        if is_server_dialback && self.server.is_some() {
            // This is our server dialing back to a client
            let handler = self
                .server
                .as_mut()
                .unwrap()
                .handle_established_outbound_connection(
                    connection_id,
                    peer,
                    addr,
                    role_override,
                    port_use,
                )?;

            _ = self.connection_info.insert(
                connection_id,
                ConnectionInfo {
                    peer_id: peer,
                    role: ConnectionRole::Server,
                },
            );

            tracing::debug!(%peer, %connection_id, "Outbound connection: server dial-back");
            Ok(Either::Right(handler))
        } else {
            // Regular client-initiated dial
            let handler = self.client.handle_established_outbound_connection(
                connection_id,
                peer,
                addr,
                role_override,
                port_use,
            )?;

            _ = self.connection_info.insert(
                connection_id,
                ConnectionInfo {
                    peer_id: peer,
                    role: ConnectionRole::Client,
                },
            );

            tracing::debug!(%peer, %connection_id, "Outbound connection: client initiated");
            Ok(Either::Left(handler))
        }
    }

    fn on_swarm_event(&mut self, event: FromSwarm<'_>) {
        self.client.on_swarm_event(event);

        if let Some(server) = &mut self.server {
            server.on_swarm_event(event);
        }

        match event {
            // Clean up tracking on connection close
            FromSwarm::ConnectionClosed(conn_closed) => {
                _ = self.connection_info.remove(&conn_closed.connection_id);

                // Clean up dial-back tracking for this peer
                // Only if this was the LAST connection to this peer
                if conn_closed.remaining_established == 0 {
                    _ = self.server_dialback_peers.remove(&conn_closed.peer_id);
                    _ = self.client_expecting_dialback.remove(&conn_closed.peer_id);

                    tracing::debug!(
                        peer_id = %conn_closed.peer_id,
                        "Cleaned up dial-back tracking - no more connections"
                    );
                }
            }
            // Clean up tracking on dial failure (before connection established)
            FromSwarm::DialFailure(dial_failure) => {
                if let Some(peer_id) = dial_failure.peer_id {
                    if self.server_dialback_peers.remove(&peer_id) {
                        tracing::debug!(
                            %peer_id,
                            "Cleaned up server dial-back tracking after dial failure"
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {
            Either::Left(client_handler_event) => {
                self.client.on_connection_handler_event(
                    peer_id,
                    connection_id,
                    client_handler_event,
                );
            }
            Either::Right(server_handler_event) => {
                if let Some(server) = &mut self.server {
                    server.on_connection_handler_event(
                        peer_id,
                        connection_id,
                        server_handler_event,
                    );
                } else {
                    tracing::warn!(
                        %peer_id,
                        %connection_id,
                        "Received server handler event but server is disabled"
                    );
                }
            }
        }
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        // First, close any pending connections
        if let Some((peer_id, conn_id)) = self.connections_to_close.pop() {
            tracing::debug!(%peer_id, %conn_id, "Closing connection");
            _ = self.connection_info.remove(&conn_id);
            return Poll::Ready(ToSwarm::CloseConnection {
                peer_id,
                connection: CloseConnection::One(conn_id),
            });
        }

        // Then emit queued events
        if let Some(event) = self.events.pop_front() {
            return Poll::Ready(ToSwarm::GenerateEvent(event));
        }

        // Poll server first (to catch dial-backs)
        if let Some(server) = &mut self.server {
            if let Poll::Ready(to_swarm) = server.poll(cx) {
                // Check if server is initiating a dial-back
                if let ToSwarm::Dial { opts } = &to_swarm {
                    if let Some(peer_id) = opts.get_peer_id() {
                        _ = self.server_dialback_peers.insert(peer_id);
                        tracing::debug!(%peer_id, "Server initiating dial-back");
                    }
                }
                return Poll::Ready(self.map_server_to_swarm(to_swarm));
            }
        }

        // Poll client
        if let Poll::Ready(to_swarm) = self.client.poll(cx) {
            let mapped = self.map_client_to_swarm(to_swarm);

            // Track when client sends request (means we expect a dial-back)
            if let ToSwarm::NotifyHandler { peer_id, .. } = &mapped {
                // Client is sending a dial request, so we expect a dial-back
                _ = self.client_expecting_dialback.insert(*peer_id);
                tracing::debug!(%peer_id, "Client sent request, expecting dial-back");

                if self.peers_with_server_support.insert(*peer_id) {
                    tracing::info!(%peer_id, "Detected peer has AutoNAT server support");
                    self.events
                        .push_back(Event::PeerHasServerSupport { peer_id: *peer_id });
                }
            }

            return Poll::Ready(mapped);
        }

        Poll::Pending
    }
}

impl Behaviour {
    fn map_client_to_swarm(
        &self,
        to_swarm: ToSwarm<
            client::Event,
            <<client::Behaviour as NetworkBehaviour>::ConnectionHandler as ConnectionHandler>::FromBehaviour,
        >,
    ) -> ToSwarm<Event, THandlerInEvent<Self>> {
        to_swarm
            .map_out(|client_event| {
                let tested_addr = client_event.tested_addr.clone();
                Event::Client {
                    tested_addr: tested_addr.clone(),
                    bytes_sent: client_event.bytes_sent,
                    server: client_event.server,
                    result: match client_event.result {
                        Ok(_addr) => TestResult::Reachable { addr: tested_addr },
                        Err(e) => TestResult::Failed {
                            error: format!("{e:?}"),
                        },
                    },
                }
            })
            .map_in(Either::Left)
    }

    fn map_server_to_swarm(
        &self,
        to_swarm: ToSwarm<server::Event, <<server::Behaviour as NetworkBehaviour>::ConnectionHandler as ConnectionHandler>::FromBehaviour>,
    ) -> ToSwarm<Event, THandlerInEvent<Self>> {
        to_swarm
            .map_out(|server_event| {
                let tested_addr = server_event.tested_addr.clone();
                Event::Server {
                    all_addrs: server_event.all_addrs,
                    tested_addr: tested_addr.clone(),
                    client: server_event.client,
                    data_amount: server_event.data_amount,
                    result: match server_event.result {
                        Ok(_) => TestResult::Reachable { addr: tested_addr },
                        Err(e) => TestResult::Failed {
                            error: format!("{e:?}"),
                        },
                    },
                }
            })
            .map_in(Either::Right)
    }
}
