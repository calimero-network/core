use libp2p::relay::client::Event;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "relay".yellow(), event);
    }
}
