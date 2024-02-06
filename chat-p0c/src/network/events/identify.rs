use color_eyre::owo_colors::OwoColorize;
use libp2p::identify;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<identify::Event> for EventLoop {
    async fn handle(&mut self, event: identify::Event) {
        debug!("{}: {:?}", "identify".yellow(), event);

        match event {
            identify::Event::Received { peer_id, info } => {
                for addr in info.listen_addrs {
                    self.swarm.behaviour_mut().kad.add_address(&peer_id, addr);
                }
            }
            _ => {}
        }
    }
}
