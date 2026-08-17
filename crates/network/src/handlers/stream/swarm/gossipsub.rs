use std::time::Instant;

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
                propagation_source,
                message_id: id,
                message,
            } => {
                // A forwarder that is missing from our subscriber table means
                // the two ends of the mesh disagree, and only we can see it.
                // See `subscription_repair` for why that state is permanent
                // and why a delivery is the evidence that resolves it.
                let _repaired: bool = self.subscription_repair.on_delivery(
                    &mut self.swarm.behaviour_mut().gossipsub,
                    &propagation_source,
                    &message.topic,
                    Instant::now(),
                );

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
