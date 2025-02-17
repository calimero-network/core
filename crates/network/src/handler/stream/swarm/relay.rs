use libp2p::relay::client::Event;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, EventLoop};

impl EventHandler<Event> for EventLoop {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "relay".yellow(), event);
    }
}
