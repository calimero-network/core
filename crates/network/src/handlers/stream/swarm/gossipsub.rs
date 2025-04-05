use libp2p::gossipsub::Event;
use owo_colors::OwoColorize;
use tracing::debug;

use super::{EventHandler, NetworkManager};
use crate::types::NetworkEvent;

impl EventHandler<Event> for NetworkManager {
    fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "gossipsub".yellow(), event);

        match event {
            Event::Message {
                message_id: id,
                message,
                ..
            } => {
                self.event_recipient
                    .do_send(NetworkEvent::Message { id, message });
            }
            Event::Subscribed { peer_id, topic } => {
                self.event_recipient
                    .do_send(NetworkEvent::Subscribed { peer_id, topic });
            }
            Event::Unsubscribed { peer_id, topic } => {
                self.event_recipient
                    .do_send(NetworkEvent::Unsubscribed { peer_id, topic });
            }
            Event::GossipsubNotSupported { .. } => {}
        }
    }
}
