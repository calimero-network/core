use actix::{Handler, Message};
use calimero_network_primitives::messages::NetworkEvent;

use crate::NodeManager;

impl Handler<NetworkEvent> for NodeManager {
    type Result = <NetworkEvent as Message>::Result;

    fn handle(&mut self, msg: NetworkEvent, _ctx: &mut Self::Context) -> Self::Result {
        match msg {
            NetworkEvent::ListeningOn {
                listener_id,
                address,
            } => todo!(),
            NetworkEvent::Subscribed { peer_id, topic } => todo!(),
            NetworkEvent::Unsubscribed { peer_id, topic } => todo!(),
            NetworkEvent::Message { id, message } => todo!(),
            NetworkEvent::StreamOpened { peer_id, stream } => todo!(),
        }
    }
}
