use color_eyre::owo_colors::OwoColorize;
use libp2p::relay;
use tracing::info;

use super::{EventHandler, EventLoop};

impl EventHandler<relay::Event> for EventLoop {
    async fn handle(&mut self, event: relay::Event) {
        info!("{}: {:?}", "relay".yellow(), event);
    }
}
