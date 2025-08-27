use libp2p::ping::Event;
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);
        debug!("{}: {:?}", "ping".yellow(), event);
    }
}
