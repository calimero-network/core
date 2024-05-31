use libp2p::mdns;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop};

impl EventHandler<mdns::Event> for EventLoop {
    async fn handle(&mut self, event: mdns::Event) {
        debug!("{}: {:?}", "mdns".yellow(), event);

        match event {
            mdns::Event::Discovered(peers) => {
                for (peer_id, addr) in peers {
                    debug!(%peer_id, %addr, "Discovered peer via mdns");

                    if let Err(err) = self.swarm.dial(addr) {
                        error!("Failed to dial peer: {:?}", err);
                    }
                }
            }
            _ => {}
        }
    }
}
