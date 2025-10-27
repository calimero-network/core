use actix::StreamHandler;
use calimero_network_primitives::messages::NetworkEvent;
use calimero_network_primitives::stream::Stream;
use libp2p::{PeerId, Stream as P2pStream, StreamProtocol};
use tracing::debug;

use crate::NetworkManager;

#[derive(Debug)]
pub struct FromIncoming(PeerId, P2pStream, StreamProtocol);

impl FromIncoming {
    pub const fn from_stream(peer_id: PeerId, stream: P2pStream, protocol: StreamProtocol) -> Self {
        Self(peer_id, stream, protocol)
    }
}

impl StreamHandler<FromIncoming> for NetworkManager {
    fn started(&mut self, _ctx: &mut Self::Context) {
        debug!("started receiving incoming connections");
    }

    fn handle(
        &mut self,
        FromIncoming(peer_id, stream, protocol): FromIncoming,
        _ctx: &mut Self::Context,
    ) {
        self.event_recipient.do_send(NetworkEvent::StreamOpened {
            peer_id,
            stream: Box::new(Stream::new(stream)),
            protocol,
        });
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        debug!("finished receiving incoming connections");
    }
}
