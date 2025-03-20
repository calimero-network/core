use libp2p::autonat::{Event, NatStatus, OutboundProbeEvent};
use libp2p::PeerId;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "autonat".yellow(), event);

        match event {
            Event::OutboundProbe(outbound_probe_event) => match outbound_probe_event {
                OutboundProbeEvent::Request { peer, .. } => {
                    debug!("Sent probe request to: {:?}", peer)
                }
                OutboundProbeEvent::Response { peer, address, .. } => {
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
                        for peer_id in &rendezvous_peers {
                            if let Err(err) = self.rendezvous_discover(peer_id) {
                                error!(%err, "Failed to perform rendezvous discovery");
                            }
                            if let Err(err) = self.rendezvous_register(peer_id) {
                                error!(%err, "Failed to register with rendezvous");
                            }
                        }
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
                        for peer_id in relay_peers {
                            if let Err(err) = self.create_relay_reservation(&peer_id) {
                                error!(%err, "Failed to handle relay reservation");
                            }
                        }

                        for peer_id in &rendezvous_peers {
                            if let Err(err) = self.rendezvous_discover(peer_id) {
                                error!(%err, "Failed to perform rendezvous discovery");
                            }
                            if let Err(err) = self.rendezvous_register(peer_id) {
                                error!(%err, "Failed to register with rendezvous");
                            }
                        }
                    }
                }
                OutboundProbeEvent::Error { .. } => {
                    error!("Outbound probe failed")
                }
            },
            Event::StatusChanged { old, new } => {
                debug!("NAT status changed from {:?} to {:?}", old, new);

                self.discovery.state.update_autonat_status(new.clone());
                if matches!(&old, NatStatus::Public(_)) && matches!(&new, NatStatus::Private) {
                    self.discovery.state.update_autonat_became_private();
                }
            }
            _ => {}
        }
    }
}
