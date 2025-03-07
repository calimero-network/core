use libp2p::autonat::Event;
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
                }
                libp2p::autonat::OutboundProbeEvent::Error { .. } => {
                    error!("Outbound probe failed")
                }
            },
            Event::StatusChanged { old, new } => {
                debug!("NAT status changed from {:?} to {:?}", old, new);
                // Should we unregister from rendezvous here if we became private?
                self.discovery.state.update_autonat_status(new);
            }
            _ => {}
        }
    }
}
