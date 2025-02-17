use libp2p::dcutr::Event;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "dcutr".yellow(), event);
    }
}
