use libp2p::identify::Event;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "identify".yellow(), event);

        if let Event::Received { peer_id, info } = event {
            self.discovery
                .state
                .update_peer_protocols(&peer_id, &info.protocols);

            if self.discovery.state.is_peer_relay(&peer_id)
                && self.discovery.state.is_autonat_status_private()
            {
                if let Err(err) = self.create_relay_reservation(&peer_id) {
                    error!(%err, "Failed to handle relay reservation");
                };
            }

            if self.discovery.state.is_peer_rendezvous(&peer_id)
                && self.discovery.state.is_autonat_status_public()
                && self.discovery.state.autonat_confidence()
                    >= self.discovery.autonat_config.confidence_threshold
            {
                if let Err(err) = self.rendezvous_discover(&peer_id) {
                    error!(%err, "Failed to perform rendezvous discovery");
                };

                if let Err(err) = self.rendezvous_register(&peer_id) {
                    error!(%err, "Failed to update registration discovery");
                };
            }
        }
    }
}
