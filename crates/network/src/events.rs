#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]

use eyre::eyre;
use libp2p::core::ConnectedPoint;
use multiaddr::Protocol;
use tracing::{error, info};

use super::*;
use crate::discovery::state::{PeerDiscoveryMechanism, RelayReservationStatus};
use crate::types::NetworkEvent;

mod autonat;
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

#[allow(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
impl EventLoop {
    // TODO: Consider splitting this long function into multiple parts.
    #[expect(clippy::too_many_lines, reason = "TODO: Will be refactored")]
    pub(super) async fn handle_swarm_event(&mut self, event: SwarmEvent<BehaviourEvent>) {
        #[expect(clippy::wildcard_enum_match_arm, reason = "This is reasonable here")]
        match event {
            SwarmEvent::Behaviour(event) => match event {
                BehaviourEvent::Autonat(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Dcutr(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Gossipsub(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Identify(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Kad(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Mdns(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Ping(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Relay(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Rendezvous(event) => EventHandler::handle(self, event).await,
                BehaviourEvent::Stream(()) => {}
            },
            SwarmEvent::NewListenAddr {
                listener_id,
                address,
            } => {
                let local_peer_id = *self.swarm.local_peer_id();
                if let Err(err) = self
                    .event_sender
                    .send(NetworkEvent::ListeningOn {
                        listener_id,
                        address: address.with(Protocol::P2p(local_peer_id)),
                    })
                    .await
                {
                    error!("Failed to send listening on event: {:?}", err);
                }
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
                if let ConnectedPoint::Dialer { .. } = endpoint {
                    self.discovery
                        .state
                        .add_peer_addr(peer_id, endpoint.get_remote_address());

                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        drop(sender.send(Ok(Some(()))));
                    }
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

                if !self.swarm.is_connected(&peer_id)
                    && !self.discovery.state.is_peer_relay(&peer_id)
                    && !self.discovery.state.is_peer_rendezvous(&peer_id)
                    && !self
                        .discovery
                        .state
                        .is_peer_discovered_via(&peer_id, PeerDiscoveryMechanism::Mdns)
                {
                    self.discovery.state.remove_peer(&peer_id);
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                debug!(?peer_id, %error, "Outgoing connection error");
                if let Some(peer_id) = peer_id {
                    if let Some(sender) = self.pending_dial.remove(&peer_id) {
                        drop(sender.send(Err(eyre!(error))));
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
                addresses, reason, ..
            } => trace!("Listener closed: {:?} {:?}", addresses, reason.err()),
            SwarmEvent::ListenerError { error, .. } => trace!("Listener error: {:?}", error),
            SwarmEvent::NewExternalAddrCandidate { address } => {
                trace!("New external address candidate: {}", address);
            }
            SwarmEvent::ExternalAddrConfirmed { address } => {
                info!("External address confirmed: {}", address);
                if let Ok(relayed_addr) = RelayedMultiaddr::try_from(&address) {
                    self.discovery.state.update_relay_reservation_status(
                        &relayed_addr.relay_peer,
                        RelayReservationStatus::Accepted,
                    );
                }
                self.broadcast_rendezvous_registrations();
            }
            SwarmEvent::ExternalAddrExpired { address } => {
                info!("External address expired: {}", address);
                if let Ok(relayed_addr) = RelayedMultiaddr::try_from(&address) {
                    self.discovery.state.update_relay_reservation_status(
                        relayed_addr.relay_peer_id(),
                        RelayReservationStatus::Expired,
                    );
                }
                self.broadcast_rendezvous_registrations();
            }
            SwarmEvent::NewExternalAddrOfPeer { peer_id, address } => {
                debug!("New external address of peer: {} {}", peer_id, address);
            }
            unhandled => warn!("Unhandled event: {:?}", unhandled),
        }
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
