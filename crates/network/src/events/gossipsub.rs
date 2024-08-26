use calimero_primitives::identity::PeerId;
use libp2p::gossipsub::Event;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{EventHandler, EventLoop};
use crate::types::{Message, NetworkEvent};

impl EventHandler<Event> for EventLoop {
    async fn handle(&mut self, event: Event) {
        debug!("{}: {:?}", "gossipsub".yellow(), event);

        match event {
            Event::Message {
                message_id: id,
                message,
                ..
            } => {
                if let Err(err) = self
                    .event_sender
                    .send(NetworkEvent::Message {
                        id,
                        message: Message::from(message),
                    })
                    .await
                {
                    error!("Failed to send message event: {:?}", err);
                }
            }
            Event::Subscribed { peer_id, topic } => {
                if (self
                    .event_sender
                    .send(NetworkEvent::Subscribed {
                        peer_id: PeerId::from(peer_id),
                        topic,
                    })
                    .await)
                    .is_err()
                {
                    error!("Failed to send subscribed event");
                }
            }
            Event::GossipsubNotSupported { .. } | Event::Unsubscribed { .. } => {}
        }
    }
}
