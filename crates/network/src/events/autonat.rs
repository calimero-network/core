use libp2p::autonat::{Event, NatStatus};
use libp2p::PeerId;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "autonat".yellow(), event);

        match event {
            Event::OutboundProbe(outbound_probe_event) => match outbound_probe_event {
                libp2p::autonat::OutboundProbeEvent::Request { peer, .. } => {
                    debug!("Sent probe request to: {:?}", peer)
                }
                libp2p::autonat::OutboundProbeEvent::Response { peer, address, .. } => {
                    debug!("Peer: {} determined external address {}", peer, address);
                    self.discovery
                        .state
                        .update_autonat_confidence(self.swarm.behaviour().autonat.confidence());

                    let rendezvous_peers: Vec<PeerId> =
                        self.discovery.state.get_rendezvous_peer_ids().collect();
                    let relay_peers: Vec<PeerId> =
                        self.discovery.state.get_relay_peer_ids().collect();

                    if self.discovery.state.is_autonat_status_public()
                        && self.discovery.state.autonat_confidence()
                            >= self.discovery.autonat_config.confidence_threshold
                    {
                        self.handle_rendezvous_operations(&rendezvous_peers);
                    }

                    if self.discovery.state.is_autonat_status_private()
                        && self.discovery.state.autonat_confidence()
                            >= self.discovery.autonat_config.confidence_threshold
                    {
                        if self.discovery.state.autonat_became_private() {
                            for peer_id in &rendezvous_peers {
                                drop(self.rendezvous_unregister(peer_id));
                            }
                        }
                        self.handle_relay_reservations(&relay_peers);

                        self.handle_rendezvous_operations(&rendezvous_peers);
                    }
                }
                libp2p::autonat::OutboundProbeEvent::Error { .. } => {
                    error!("Outbound probe failed")
                }
            },
            Event::StatusChanged { old, new } => {
                debug!("NAT status changed from {:?} to {:?}", old, new);
                // Should we unregister from rendezvous here if we became private?
                // If we were public, that means we were registered on the rendezvous node (probably, if we had enough confirmations),
                // which means our address there isn't reachable anymore
                // If we get enough confirmations that we are really now private, we should unregister the "old" external addresses
                // But also with enough confimations that we are now private, we will request a relay reservation and we will have a valid
                // external address (the relayed one)
                // And then we will register the relayed one to the rendezvous node

                self.discovery.state.update_autonat_status(new.clone());
                if matches!(&old, NatStatus::Public(_)) && matches!(&new, NatStatus::Private) {
                    self.discovery.state.update_autonat_became_private(); // now I can check if it was a switch, when we confirm, we unregister from rendezvous
                }
            }
            _ => {}
        }
    }
}

#[allow(
    clippy::multiple_inherent_impl,
    reason = "Currently necessary due to code structure"
)]
impl EventLoop {
    fn handle_rendezvous_operations(&mut self, rendezvous_peers: &[PeerId]) {
        for peer_id in rendezvous_peers {
            if let Err(err) = self.rendezvous_discover(peer_id) {
                error!(%err, "Failed to perform rendezvous discovery");
            }
            if let Err(err) = self.rendezvous_register(peer_id) {
                error!(%err, "Failed to register with rendezvous");
            }
        }
    }

    fn handle_relay_reservations(&mut self, peers: &[PeerId]) {
        for peer_id in peers {
            if let Err(err) = self.create_relay_reservation(&peer_id) {
                error!(%err, "Failed to handle relay reservation");
            }
        }
    }
}
