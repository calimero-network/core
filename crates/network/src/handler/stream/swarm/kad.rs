use libp2p::kad::{Event, QueryResult};
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "kad".yellow(), event);

        match event {
            Event::OutboundQueryProgressed {
                id,
                result: QueryResult::Bootstrap(result),
                ..
            } => {
                if let Some(sender) = self.pending_bootstrap.remove(&id) {
                    drop(sender.send(result.map(|_| None).map_err(Into::into)));
                }
            }
            Event::InboundRequest { .. }
            | Event::OutboundQueryProgressed { .. }
            | Event::ModeChanged { .. }
            | Event::PendingRoutablePeer { .. }
            | Event::RoutablePeer { .. }
            | Event::RoutingUpdated { .. }
            | Event::UnroutablePeer { .. } => {}
        }
    }
}
