use libp2p::autonat::{Event, OutboundProbeEvent};
use owo_colors::OwoColorize;
use tracing::{debug, error, info};

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "autonat".yellow(), event);

        match event {
            Event::OutboundProbe(outbound_probe_event) => match outbound_probe_event {
                OutboundProbeEvent::Request { peer, .. } => {
                    debug!(%peer, "Sent probe request");
                }
                OutboundProbeEvent::Response { peer, address, .. } => {
                    info!(%peer, %address, "Peer determined our external address");

                    // TODO: Revisit AutoNAT protocol integration
                    // let rendezvous_peers: Vec<PeerId> =
                    //     self.discovery.state.get_rendezvous_peer_ids().collect();
                    // let relay_peers: Vec<PeerId> =
                    //     self.discovery.state.get_relay_peer_ids().collect();

                    // if self.swarm.behaviour().autonat.confidence()
                    //     >= self.discovery.autonat_config.confidence_threshold
                    // {
                    //     return;
                    // }

                    // if self.discovery.state.is_autonat_status_public() {
                    //     for peer_id in &rendezvous_peers {
                    //         if let Err(err) = self.rendezvous_discover(peer_id) {
                    //             error!(%err, "Failed to perform rendezvous discovery");
                    //         }
                    //         if let Err(err) = self.rendezvous_register(peer_id) {
                    //             error!(%err, "Failed to register with rendezvous");
                    //         }
                    //     }
                    // }

                    // if self.discovery.state.is_autonat_status_private() {
                    //     if self.discovery.state.autonat_became_private() {
                    //         for peer_id in &rendezvous_peers {
                    //             drop(self.rendezvous_unregister(peer_id));
                    //         }
                    //     }
                    //     for peer_id in relay_peers {
                    //         if let Err(err) = self.create_relay_reservation(&peer_id) {
                    //             error!(%err, "Failed to handle relay reservation");
                    //         }
                    //     }

                    //     for peer_id in &rendezvous_peers {
                    //         if let Err(err) = self.rendezvous_discover(peer_id) {
                    //             error!(%err, "Failed to perform rendezvous discovery");
                    //         }
                    //         if let Err(err) = self.rendezvous_register(peer_id) {
                    //             error!(%err, "Failed to register with rendezvous");
                    //         }
                    //     }
                    // }
                }
                OutboundProbeEvent::Error { .. } => {
                    error!("Outbound probe failed")
                }
            },
            Event::StatusChanged { old, new } => {
                debug!("NAT status changed from {:?} to {:?}", old, new);

                self.discovery.state.update_autonat_status(new);
            }
            _ => {}
        }
    }
}
