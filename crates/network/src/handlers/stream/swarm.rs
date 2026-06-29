use actix::{AsyncContext, StreamHandler};
use calimero_network_primitives::messages::NetworkEvent;
use eyre::eyre;
use libp2p::swarm::{DialError, SwarmEvent};
use libp2p::PeerId;
use multiaddr::{Multiaddr, Protocol};
use tracing::{debug, info, trace, warn};

use crate::behaviour::BehaviourEvent;
use crate::discovery::state::{PeerDiscoveryMechanism, RelayReservationStatus};
use crate::NetworkManager;

mod autonat;
mod dcutr;
mod gossipsub;
mod identify;
mod kad;
mod mdns;
mod ping;
mod relay;
mod rendezvous;
mod specialized_node_invite;

pub trait EventHandler<E> {
    fn handle(&mut self, event: E);
}

impl From<SwarmEvent<BehaviourEvent>> for FromSwarm {
    fn from(event: SwarmEvent<BehaviourEvent>) -> Self {
        Self(event)
    }
}

#[derive(Debug)]
pub struct FromSwarm(SwarmEvent<BehaviourEvent>);

impl StreamHandler<FromSwarm> for NetworkManager {
    fn started(&mut self, _ctx: &mut Self::Context) {
        debug!("started receiving swarm messages");
    }

    #[expect(clippy::too_many_lines, reason = "Enum with many variants")]
    fn handle(&mut self, FromSwarm(event): FromSwarm, ctx: &mut Self::Context) {
        #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
        match event {
            SwarmEvent::Behaviour(event) => match event {
                BehaviourEvent::Autonat(event) => EventHandler::handle(self, event),
                BehaviourEvent::Dcutr(event) => EventHandler::handle(self, event),
                BehaviourEvent::Gossipsub(event) => EventHandler::handle(self, event),
                BehaviourEvent::Identify(event) => EventHandler::handle(self, event),
                BehaviourEvent::Kad(event) => EventHandler::handle(self, event),
                BehaviourEvent::Mdns(event) => EventHandler::handle(self, event),
                BehaviourEvent::Ping(event) => EventHandler::handle(self, event),
                BehaviourEvent::Relay(event) => EventHandler::handle(self, event),
                BehaviourEvent::Rendezvous(event) => EventHandler::handle(self, event),
                BehaviourEvent::Stream(()) => {}
                BehaviourEvent::SpecializedNodeInvite(event) => EventHandler::handle(self, event),
            },
            SwarmEvent::NewListenAddr {
                listener_id,
                address,
            } => {
                let local_peer_id = *self.swarm.local_peer_id();
                let _ignored = self.event_dispatcher.dispatch(NetworkEvent::ListeningOn {
                    listener_id,
                    address: address.with(Protocol::P2p(local_peer_id)),
                });
            }
            SwarmEvent::IncomingConnection {
                connection_id,
                local_addr,
                send_back_addr,
            } => {
                debug!(
                    ?connection_id,
                    ?local_addr,
                    ?send_back_addr,
                    "Incoming connection"
                );
            }
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                debug!(%peer_id, ?endpoint, "Connection established");

                // Record the remote address for both inbound and outbound
                // connections. The previous version only recorded for the
                // Dialer endpoint, which meant a peer that dialed us first
                // never made it into our address book until rendezvous
                // happened to surface them later. That left the rendezvous-
                // tick re-dial loop with nothing to iterate on
                // ConnectionClosed.
                //
                // Skip relayed multiaddrs (`/p2p-circuit/`) — those aren't
                // useful as direct-dial entries for the peer, and storing
                // them would pollute the book with addresses that always
                // fail to dial directly.
                let remote = endpoint.get_remote_address();
                let is_relayed = remote.iter().any(|p| matches!(p, Protocol::P2pCircuit));
                if !is_relayed {
                    self.discovery.state.add_peer_addr(peer_id, remote);
                }

                // Record into the persistent peer cache regardless of
                // relayed-ness — a relayed circuit address is still
                // re-dialable after a restart if the peer re-reserves on
                // the same relay, so it's worth caching for NAT'd
                // co-members (the discovery book above keeps direct only).
                let cache_addr = remote.clone();
                self.record_connected_addr(peer_id, cache_addr);

                // Resolve any pending dial for this peer on *any* established
                // connection, not just the `Dialer` endpoint. `pending_dial`
                // is only ever populated by the `Dial` handler when we call
                // `swarm.dial()`, so resolving on any endpoint is safe: a
                // purely inbound connection cannot carry a `pending_dial`
                // entry unless we also initiated a dial to that peer (the
                // simultaneous-open case this fix targets). A dial we
                // initiated can complete as a `Listener` endpoint (e.g. a
                // simultaneous-open / hole-punched connection where the peer's
                // SYN wins the race), in which case scoping the clear to
                // `Dialer` would strand the `pending_dial` entry — and its
                // oneshot sender — until shutdown. That leak both wedges the
                // awaiting `Dial` future indefinitely and, when the swarm is
                // finally torn down, drops the sender and surfaces as a panic.
                if let Some(sender) = self.pending_dial.remove(&peer_id) {
                    let _ignored = sender.send(Ok(()));
                }
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                endpoint,
                num_established,
                cause,
            } => {
                debug!(
                    is_connected=%self.swarm.is_connected(&peer_id),
                    %peer_id,
                    ?connection_id,
                    ?endpoint,
                    %num_established,
                    ?cause,
                    "Connection closed",
                );

                // Drop any ping-failure tally for this connection. The entry
                // is normally already gone (cleared on a ping success, or by
                // the ping handler right before it forced this close), but
                // closes triggered by anything other than our ping watchdog
                // (peer-initiated, transport error, restart) would otherwise
                // leak a stale counter keyed on a connection id that will
                // never be reused.
                let _stale = self.ping_failures.remove(&connection_id);

                if !self.swarm.is_connected(&peer_id) {
                    // Two mutually-exclusive branches keyed on the role
                    // of the disconnected peer:
                    //
                    //   1. Relay peer: reservation we held is gone with
                    //      the control connection. Mark Expired and
                    //      queue recovery. `on_relay_reservation_lost`
                    //      is idempotent for already-Expired peers, so
                    //      it's safe to call multiple times for the
                    //      same disconnect cascade.
                    //   2. Regular calimero peer (not relay, not
                    //      rendezvous, not mdns-discovered): the peer
                    //      may have restarted with a fresh libp2p
                    //      identity state. Issue a throttle-bypassed
                    //      rendezvous re-query to discover the new
                    //      registration. See the inline comment on
                    //      `for delay_secs in ...` for the retry
                    //      schedule rationale.
                    //
                    // The branches were previously a duplicated
                    // `is_peer_relay` predicate plus a long
                    // `&& !is_peer_relay && !is_peer_rendezvous && !mdns`
                    // compound on the regular branch. Restructured as
                    // explicit if/else if to make the mutual exclusion
                    // visible and survive future role-classification
                    // refactors without the predicates drifting.
                    if self.discovery.state.is_peer_relay(&peer_id) {
                        let actions = self.discovery.state.on_relay_reservation_lost(&peer_id);
                        self.execute_reachability_actions(actions);
                    } else if !self.discovery.state.is_peer_rendezvous(&peer_id)
                        && !self
                            .discovery
                            .state
                            .is_peer_discovered_via(&peer_id, PeerDiscoveryMechanism::Mdns)
                    {
                        self.discovery.state.remove_peer(&peer_id);

                        // Our address book held nothing dialable for
                        // them — relayed addresses are intentionally
                        // not stored (see `swarm.rs
                        // ConnectionEstablished` filter on
                        // `Protocol::P2pCircuit`), so the only way to
                        // pick up the post-restart registration is to
                        // re-query rendezvous. The periodic tick is
                        // gated by `discovery_rpm` (default 0.5 →
                        // 120s floor), which is far longer than the
                        // 120s sync-recovery budget the upstream
                        // workflows assume. Issue the discover via
                        // the force-path here so the throttle gets
                        // bypassed for this event-driven case.
                        let actions = self.discovery.state.on_regular_peer_disconnected();
                        self.execute_reachability_actions(actions);

                        // Delayed re-fires. The immediate discover
                        // above usually misses the target: a container
                        // restart of the disconnected peer takes ~3-5s
                        // for the new merod process to come up, dial
                        // the rendezvous server, and re-register its
                        // record. Our immediate query lands in that
                        // gap and the rendezvous server returns only
                        // ourselves (the peer's old registration was
                        // evicted when the relay control connection
                        // closed; the new one hasn't been written yet).
                        //
                        // Four re-fires at 5s / 15s / 30s / 60s cover
                        // the typical restart-window observed in CI
                        // (~3-5s warm, up to ~15-20s on a contended
                        // runner, up to ~60s on a cold-cache + I/O-
                        // saturated runner). Each re-fire bypasses
                        // the throttle via the force-path. The cost
                        // is bounded — at most 5 boot-node queries
                        // per (disconnect_event, rendezvous_peer)
                        // pair, i.e. 5 queries per disconnect in the
                        // single-rendezvous case (production today)
                        // and 5×M queries per disconnect with M
                        // rendezvous peers configured (multi-
                        // rendezvous deployments). No queries at all
                        // if no rendezvous peers are configured
                        // (mdns-only deployments).
                        //
                        // After the +60s retry the periodic
                        // discovery tick (default 15s, see
                        // `RendezvousConfig::default()`) takes over.
                        // It is throttled at one query per 120s by
                        // default, so worst-case rediscover latency
                        // after a missed retry window is bounded by
                        // `last_force_fire + 120s` ≈ t+180s — bad
                        // for a 120s test budget, fine for a
                        // production deployment where the peer is
                        // expected back at all.
                        //
                        // Each closure captures `disconnected_peer`
                        // and gates the re-fire on
                        // `!is_connected(&disconnected_peer)`. If
                        // the peer is currently connected at re-fire
                        // time (fast container restart, transient
                        // TCP RST that recovered), the re-fire is a
                        // no-op — no wasted boot-node query, no
                        // spurious force-discover. Note: this is a
                        // point-in-time check at fire-time; a peer
                        // that reconnects and then disconnects again
                        // before the next re-fire WILL trigger a
                        // force-discover at that fire (because
                        // `is_connected` would be false again),
                        // which is the desired behavior — the second
                        // disconnect is a fresh event worth
                        // recovering from. This also bounds the
                        // multi-peer case: if peer A disconnects and
                        // peer B disconnects 1s later, A's later
                        // re-fires don't double-fire for B (B has
                        // its own re-fires; A's are gated on A
                        // specifically).
                        let disconnected_peer = peer_id;
                        for delay_secs in [5_u64, 15, 30, 60] {
                            ctx.run_later(
                                core::time::Duration::from_secs(delay_secs),
                                move |actor, _ctx| {
                                    if actor.swarm.is_connected(&disconnected_peer) {
                                        return;
                                    }
                                    let actions =
                                        actor.discovery.state.on_regular_peer_disconnected();
                                    actor.execute_reachability_actions(actions);
                                },
                            );
                        }
                    }
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                debug!(?peer_id, %error, "Outgoing connection error");

                // Attribute the failure to specific addresses where DialError
                // gives us that detail. Three variants carry addresses:
                //   - LocalPeerId { address }: we dialed our own peer id at
                //     that address; whoever advertised it for this peer was
                //     wrong, so evict.
                //   - WrongPeerId { address, .. }: the peer at that address
                //     turned out to have a different identity; same idea.
                //   - Transport(Vec<(addr, err)>): each address we tried in
                //     this dial, with the transport-level error per attempt.
                // Other variants (NoAddresses, Aborted, Denied, DialPeer
                // ConditionFalse) don't pin the failure to an address, so
                // there's nothing to attribute.
                if let Some(peer_id) = peer_id {
                    let failed_addrs: Vec<&Multiaddr> = match &error {
                        DialError::LocalPeerId { address }
                        | DialError::WrongPeerId { address, .. } => vec![address],
                        DialError::Transport(attempts) => {
                            attempts.iter().map(|(addr, _)| addr).collect()
                        }
                        _ => vec![],
                    };
                    for addr in failed_addrs {
                        let _ = self.discovery.state.record_dial_failure(&peer_id, addr);
                    }

                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        let _ignored = sender.send(Err(eyre!(error)));
                    }
                }
            }
            SwarmEvent::IncomingConnectionError {
                send_back_addr,
                error,
                ..
            } => {
                debug!(?send_back_addr, %error, "Incoming connection error");
            }
            SwarmEvent::Dialing {
                peer_id: Some(peer_id),
                ..
            } => debug!("Dialing peer: {}", peer_id),
            SwarmEvent::ExpiredListenAddr { address, .. } => {
                trace!("Expired listen address: {}", address);
            }
            SwarmEvent::ListenerClosed {
                listener_id,
                addresses,
                reason,
                ..
            } => {
                trace!(
                    ?listener_id,
                    ?addresses,
                    error = ?reason.err(),
                    "Listener closed"
                );

                // Prefer the registered listener-id mapping: when
                // create_relay_reservation calls listen_on, we record the
                // returned ListenerId against the relay peer. This routes
                // recovery correctly even when libp2p emits
                // ListenerClosed with `addresses: []` — which it does when
                // the relay denies a reservation before any external
                // address gets allocated (e.g. quota wall, rate limit).
                // The addresses-iteration fallback below would silently
                // miss that case.
                //
                // `take_relay_listener` removes the entry as it returns
                // the peer, so the map cannot leak entries across
                // reservation cycles regardless of which branch handles
                // the close event.
                //
                // The fallback iterates addresses for listeners we did
                // not register ourselves (defensive — covers any future
                // code path that calls listen_on with a relayed multiaddr
                // outside create_relay_reservation). It does not need to
                // clean the map: by definition the map had no entry to
                // clean.
                //
                // The swarm typically also emits ExternalAddrExpired for
                // the same address, so the second call into
                // on_relay_reservation_lost will be a no-op (status
                // already Expired).
                if let Some(relay_peer) = self.discovery.state.take_relay_listener(&listener_id) {
                    let actions = self.discovery.state.on_relay_reservation_lost(&relay_peer);
                    self.execute_reachability_actions(actions);
                } else {
                    for address in &addresses {
                        if let Ok(relayed_addr) = RelayedMultiaddr::try_from(address) {
                            let actions = self
                                .discovery
                                .state
                                .on_relay_reservation_lost(relayed_addr.relay_peer_id());
                            self.execute_reachability_actions(actions);
                        }
                    }
                }
            }
            SwarmEvent::ListenerError { error, .. } => trace!("Listener error: {:?}", error),
            SwarmEvent::NewExternalAddrCandidate { address } => {
                trace!("New external address candidate: {}", address);
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                info!("Swarm: External address confirmed: {}", address);

                // Check if this is a relay address and update relay metadata
                let is_relay_address =
                    if let Ok(relayed_addr) = RelayedMultiaddr::try_from(&address) {
                        self.discovery.state.update_relay_reservation_status(
                            &relayed_addr.relay_peer,
                            RelayReservationStatus::Accepted,
                        );
                        true
                    } else {
                        false
                    };

                // Update our reachability state only for direct (non-relay) addresses
                // Relay addresses don't make us "publicly reachable" - we're still behind NAT
                // and shouldn't enable AutoNAT server (we can't perform dial-backs for NAT tests)
                if !is_relay_address {
                    let actions = self.discovery.state.on_address_confirmed(&address);
                    self.execute_reachability_actions(actions);
                }

                self.broadcast_rendezvous_registrations();
            }
            SwarmEvent::ExternalAddrExpired { address } => {
                info!("Swarm: External address expired: {}", address);

                if let Ok(relayed_addr) = RelayedMultiaddr::try_from(&address) {
                    // Relay reservation lapsed (renewal failed, relay disconnected,
                    // or max_circuit_duration exceeded). Mark Expired and queue a
                    // re-request so we don't sit silently unreachable until restart.
                    let actions = self
                        .discovery
                        .state
                        .on_relay_reservation_lost(relayed_addr.relay_peer_id());
                    self.execute_reachability_actions(actions);
                } else {
                    // Direct (non-relay) address: update reachability state.
                    // Must handle here due to libp2p bug #6203 — AutoNAT does not
                    // retest expired addresses.
                    let actions = self.discovery.state.on_address_removed(&address);
                    self.execute_reachability_actions(actions);
                }

                self.broadcast_rendezvous_registrations();
            }
            SwarmEvent::NewExternalAddrOfPeer { peer_id, address } => {
                debug!("New external address of peer: {} {}", peer_id, address);
            }
            unhandled => warn!("Unhandled event: {:?}", unhandled),
        }
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        debug!("finished receiving swarm messages");
    }
}

#[derive(Debug)]
pub struct RelayedMultiaddr {
    relay_peer: PeerId,
}

impl TryFrom<&Multiaddr> for RelayedMultiaddr {
    type Error = &'static str;

    fn try_from(value: &Multiaddr) -> Result<Self, Self::Error> {
        let mut peer_ids = Vec::new();

        let mut iter = value.iter();

        while let Some(protocol) = iter.next() {
            #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
            match protocol {
                Protocol::P2pCircuit => {
                    if peer_ids.is_empty() {
                        return Err("expected at least one p2p proto before P2pCircuit");
                    }
                    let Some(Protocol::P2p(id)) = iter.next() else {
                        return Err("expected p2p proto after P2pCircuit");
                    };
                    peer_ids.push(id);
                }
                Protocol::P2p(id) => {
                    peer_ids.push(id);
                }
                _ => {}
            }
        }

        if peer_ids.len() < 2 {
            return Err("expected at least two p2p protos, one for peer and one for relay");
        }

        Ok(Self {
            relay_peer: peer_ids.remove(0),
        })
    }
}

impl RelayedMultiaddr {
    const fn relay_peer_id(&self) -> &PeerId {
        &self.relay_peer
    }
}
