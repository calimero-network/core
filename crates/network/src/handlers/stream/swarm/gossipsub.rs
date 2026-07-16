use calimero_network_primitives::messages::NetworkEvent;
use libp2p::gossipsub::Event;
use libp2p_metrics::Recorder;
use tracing::{debug, warn};

use super::{EventHandler, NetworkManager};

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        self.metrics.record(&event);

        match event {
            Event::Message {
                message_id: id,
                message,
                ..
            } => {
                // Log only non-sensitive metadata. The previous `{:?}` of the
                // whole event dumped the raw `message.data` payload on a hot
                // path (data leak) and injected ANSI color codes into the logs.
                debug!(
                    target: "network::gossipsub",
                    message_id = ?id,
                    source = ?message.source,
                    topic = ?message.topic,
                    payload_len = message.data.len(),
                    "gossipsub message received"
                );
                if !self
                    .event_dispatcher
                    .dispatch(NetworkEvent::Message { id, message })
                {
                    warn!("Failed to dispatch gossipsub message event");
                }
            }
            Event::Subscribed { peer_id, topic } => {
                debug!(target: "network::gossipsub", %peer_id, ?topic, "subscribed");
                if !self
                    .event_dispatcher
                    .dispatch(NetworkEvent::Subscribed { peer_id, topic })
                {
                    warn!("Failed to dispatch subscribed event");
                }
            }
            Event::Unsubscribed { peer_id, topic } => {
                debug!(target: "network::gossipsub", %peer_id, ?topic, "unsubscribed");
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
