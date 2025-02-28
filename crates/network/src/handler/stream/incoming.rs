use actix::StreamHandler;
use libp2p::{PeerId, Stream as P2pStream};
use tracing::debug;

use crate::stream::Stream;
use crate::types::NetworkEvent;
use crate::NetworkManager;

#[derive(Debug)]
pub struct FromIncoming(PeerId, P2pStream);

impl From<(PeerId, P2pStream)> for FromIncoming {
    fn from((id, stream): (PeerId, P2pStream)) -> Self {
        Self(id, stream)
    }
}

impl StreamHandler<FromIncoming> for NetworkManager {
    fn started(&mut self, _ctx: &mut Self::Context) {
        debug!("started receiving incoming connections");
    }

    fn handle(&mut self, FromIncoming(peer_id, stream): FromIncoming, _ctx: &mut Self::Context) {
        self.event_recipient.do_send(NetworkEvent::StreamOpened {
            peer_id,
            stream: Box::new(Stream::new(stream)),
        });
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        debug!("finished receiving incoming connections");
    }
}
