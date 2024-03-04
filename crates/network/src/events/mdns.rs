use libp2p::mdns;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<mdns::Event> for EventLoop {
    async fn handle(&mut self, event: mdns::Event) {
        debug!("{}: {:?}", "mdns".yellow(), event);

        match event {
            mdns::Event::Discovered(peers) => {
                for (peer_id, addr) in peers {
                    debug!("Discovered {} at {}", peer_id, addr);

                    self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                }
            }
            mdns::Event::Expired(peers) => {
                for (peer_id, addr) in peers {
                    debug!("Expired {} at {}", peer_id, addr);

                    self.swarm
                        .behaviour_mut()
                        .kad
                        .remove_address(&peer_id, &addr);
                }
            }
        }
    }
}
