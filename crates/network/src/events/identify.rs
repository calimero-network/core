use libp2p::identify;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop};

impl EventHandler<identify::Event> for EventLoop {
    async fn handle(&mut self, event: identify::Event) {
        debug!("{}: {:?}", "identify".yellow(), event);

        match event {
            identify::Event::Received { peer_id, info } => {
                self.discovery_state
                    .update_peer_protocols(&peer_id, info.protocols);

                if self.discovery_state.is_peer_relay(&peer_id) {
                    if let Err(err) = self.create_relay_reservation(&peer_id) {
                        error!(%err, "Failed to handle relay reservation");
                    };
                }

                if self.discovery_state.is_peer_rendezvous(&peer_id) {
                    if let Err(err) = self.perform_rendezvous_discovery(&peer_id) {
                        error!(%err, "Failed to perform rendezvous discovery");
                    };

                    if let Err(err) = self.update_rendezvous_registration(&peer_id) {
                        error!(%err, "Failed to update registration discovery");
                    };
                }
            }
            _ => {}
        }
    }
}
