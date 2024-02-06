use color_eyre::owo_colors::OwoColorize;
use libp2p::ping;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<ping::Event> for EventLoop {
    async fn handle(&mut self, event: ping::Event) {
        debug!("{}: {:?}", "ping".yellow(), event);
    }
}
