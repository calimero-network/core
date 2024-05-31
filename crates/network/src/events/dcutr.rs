use libp2p::dcutr;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<dcutr::Event> for EventLoop {
    async fn handle(&mut self, event: dcutr::Event) {
        debug!("{}: {:?}", "dcutr".yellow(), event);
    }
}
