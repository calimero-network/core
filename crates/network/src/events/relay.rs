use libp2p::relay;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<relay::client::Event> for EventLoop {
    async fn handle(&mut self, event: relay::client::Event) {
        debug!("{}: {:?}", "relay".yellow(), event);
    }
}
