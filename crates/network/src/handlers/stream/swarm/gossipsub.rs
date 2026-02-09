use calimero_network_primitives::messages::NetworkEvent;
use libp2p::gossipsub::Event;
use libp2p_metrics::Recorder;
use owo_colors::OwoColorize;
use tracing::{debug, warn};

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);
        debug!("{}: {:?}", "gossipsub".yellow(), event);

        match event {
            Event::Message {
                message_id: id,
                message,
                ..
            } => {
                if !self
                    .event_dispatcher
                    .dispatch(NetworkEvent::Message { id, message })
                {
                    warn!("Failed to dispatch gossipsub message event");
                }
            }
            Event::Subscribed { peer_id, topic } => {
                if !self
                    .event_dispatcher
                    .dispatch(NetworkEvent::Subscribed { peer_id, topic })
                {
                    warn!("Failed to dispatch subscribed event");
                }
            }
            Event::Unsubscribed { peer_id, topic } => {
                if !self
                    .event_dispatcher
                    .dispatch(NetworkEvent::Unsubscribed { peer_id, topic })
                {
                    warn!("Failed to dispatch unsubscribed event");
                }
            }
            Event::GossipsubNotSupported { .. } => {}
            Event::SlowPeer { .. } => {}
        }
    }
}
