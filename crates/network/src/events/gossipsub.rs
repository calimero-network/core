use libp2p::gossipsub;
use owo_colors::OwoColorize;
use tracing::{debug, error};

use super::{types, EventHandler, EventLoop};

impl EventHandler<gossipsub::Event> for EventLoop {
    async fn handle(&mut self, event: gossipsub::Event) {
        debug!("{}: {:?}", "gossipsub".yellow(), event);

        match event {
            gossipsub::Event::Message {
                message_id: id,
                message,
                ..
            } => {
                if let Err(err) = self
                    .event_sender
                    .send(types::NetworkEvent::Message { id, message })
                    .await
                {
                    error!("Failed to send message event: {:?}", err);
                }
            }
            gossipsub::Event::Subscribed { peer_id, topic } => {
                if let Err(_) = self
                    .event_sender
                    .send(types::NetworkEvent::Subscribed { peer_id, topic })
                    .await
                {
                    error!("Failed to send subscribed event");
                }
            }
            _ => {}
        }
    }
}
