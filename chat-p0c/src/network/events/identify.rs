use color_eyre::owo_colors::OwoColorize;
use libp2p::identify;
use tracing::info;

use super::{EventHandler, EventLoop};

impl EventHandler<identify::Event> for EventLoop {
    async fn handle(&mut self, event: identify::Event) {
        info!("{}: {:?}", "identify".yellow(), event);
    }
}
