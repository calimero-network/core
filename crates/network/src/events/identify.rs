use libp2p::identify::Event;
use owo_colors::OwoColorize;
use tracing::{debug, error, info};

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "identify".yellow(), event);

        if let Event::Received { peer_id, info } = event {
            self.discovery
                .state
                .update_peer_protocols(&peer_id, &info.protocols);

            // TODO: Revist AutoNAT protocol implementation
            // if self.discovery.state.is_peer_autonat(&peer_id) {
            //     if let Err(err) = self.add_autonat_server(&peer_id) {
            //         error!(%err, "Failed to add autonat server");
            //     };
            // }

            if self.discovery.advertise_address {
                info!("Adding external address: {:?}", info.observed_addr);
                self.swarm.add_external_address(info.observed_addr);
            } else {
                if self.discovery.state.is_peer_relay(&peer_id) {
                    if let Err(err) = self.create_relay_reservation(&peer_id) {
                        error!(%err, "Failed to handle relay reservation");
                    };
                }
            }

            if self.discovery.state.is_peer_rendezvous(&peer_id) {
                if let Err(err) = self.rendezvous_discover(&peer_id) {
                    error!(%err, "Failed to perform rendezvous discovery");
                };
            }
        }
    }
}
