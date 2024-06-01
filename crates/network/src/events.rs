use tracing::error;

use super::*;

mod dcutr;
mod gossipsub;
mod identify;
mod kad;
mod mdns;
mod ping;
mod relay;
mod rendezvous;

pub trait EventHandler<E> {
    async fn handle(&mut self, event: E);
}

impl EventLoop {
    pub(super) async fn handle_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        match event {
            SwarmEvent::Behaviour(event) => match event {
                BehaviourEvent::Dcutr(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Gossipsub(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Identify(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Kad(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Mdns(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Ping(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Relay(event) => events::EventHandler::handle(self, event).await,
                BehaviourEvent::Rendezvous(event) => {
                    events::EventHandler::handle(self, event).await
                }
            },
            SwarmEvent::NewListenAddr {
                listener_id,
                address,
            } => {
                let local_peer_id = *self.swarm.local_peer_id();
                if let Err(err) = self
                    .event_sender
                    .send(types::NetworkEvent::ListeningOn {
                        listener_id,
                        address: address.with(multiaddr::Protocol::P2p(local_peer_id)),
                    })
                    .await
                {
                    error!("Failed to send listening on event: {:?}", err);
                }
            }
            SwarmEvent::IncomingConnection { .. } => {}
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                debug!(%peer_id, ?endpoint, "Connection established");
                match endpoint {
                    libp2p::core::ConnectedPoint::Dialer { .. } => {
                        self.discovery
                            .state
                            .add_peer_addr(peer_id, endpoint.get_remote_address());

                        if let Some(sender) = self.pending_dial.remove(&peer_id) {
                            let _ = sender.send(Ok(Some(())));
                        }
                    }
                    _ => {}
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
                    "Connection closed: {} {:?} {:?} {} {:?}",
                    peer_id, connection_id, endpoint, num_established, cause
                );
                if !self.swarm.is_connected(&peer_id)
                    && !self.discovery.state.is_peer_relay(&peer_id)
                    && !self.discovery.state.is_peer_rendezvous(&peer_id)
                {
                    self.discovery.state.remove_peer(&peer_id);
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                debug!(?peer_id, %error, "Outgoing connection error");
                if let Some(peer_id) = peer_id {
                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        let _ = sender.send(Err(eyre::eyre!(error)));
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
                trace!("Expired listen address: {}", address)
            }
            SwarmEvent::ListenerClosed {
                addresses, reason, ..
            } => trace!("Listener closed: {:?} {:?}", addresses, reason.err()),
            SwarmEvent::ListenerError { error, .. } => trace!("Listener error: {:?}", error),
            SwarmEvent::NewExternalAddrCandidate { address } => {
                trace!("New external address candidate: {}", address)
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                debug!("External address confirmed: {}", address);
                if let Ok(relayed_addr) = RelayedMultiaddr::try_from(&address) {
                    self.discovery.state.update_relay_reservation_status(
                        &relayed_addr.relay_peer,
                        discovery::state::RelayReservationStatus::Accepted,
                    );
                    self.discovery.state.set_pending_addr_changes();
                    if let Err(err) = self.broadcast_rendezvous_registrations() {
                        error!(%err, "Failed to handle rendezvous register");
                    };
                }
            }
            SwarmEvent::ExternalAddrExpired { address } => {
                debug!("External address expired: {}", address);
                if let Ok(relayed_addr) = RelayedMultiaddr::try_from(&address) {
                    self.discovery.state.update_relay_reservation_status(
                        &relayed_addr.relay_peer_id(),
                        discovery::state::RelayReservationStatus::Expired,
                    );

                    self.discovery.state.set_pending_addr_changes();
                    if let Err(err) = self.broadcast_rendezvous_registrations() {
                        error!(%err, "Failed to handle rendezvous register");
                    };
                }
            }
            unhandled => warn!("Unhandled event: {:?}", unhandled),
        }
    }
}

#[derive(Debug)]
pub(crate) struct RelayedMultiaddr {
    relay_peer: PeerId,
}

impl TryFrom<&Multiaddr> for RelayedMultiaddr {
    type Error = &'static str;

    fn try_from(value: &Multiaddr) -> Result<Self, Self::Error> {
        let mut peer_ids = Vec::new();

        let mut iter = value.iter();

        while let Some(protocol) = iter.next() {
            match protocol {
                multiaddr::Protocol::P2pCircuit => {
                    if peer_ids.is_empty() {
                        return Err("expected at least one p2p proto before P2pCircuit");
                    }
                    let Some(multiaddr::Protocol::P2p(id)) = iter.next() else {
                        return Err("expected p2p proto after P2pCircuit");
                    };
                    peer_ids.push(id);
                }
                multiaddr::Protocol::P2p(id) => {
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
    fn relay_peer_id(&self) -> &PeerId {
        &self.relay_peer
    }
}
