use actix::{Message, StreamHandler};
use libp2p::{PeerId, Stream as P2pStream};

use crate::stream::Stream;
use crate::types::NetworkEvent;
use crate::EventLoop;

// pub mod stream;

#[derive(Message)]
#[rtype(result = "()")]
pub struct FromIncoming(Option<(PeerId, P2pStream)>);

impl From<Option<(PeerId, P2pStream)>> for FromIncoming {
    fn from(peer_stream: Option<(PeerId, P2pStream)>) -> Self {
        FromIncoming(peer_stream)
    }
}

impl StreamHandler<FromIncoming> for EventLoop {
    fn started(&mut self, _ctx: &mut Self::Context) {
        println!("started receiving swarm messages");
    }

    fn handle(&mut self, FromIncoming(incoming_stream): FromIncoming, _ctx: &mut Self::Context) {
        let incoming_stream = incoming_stream.expect("Incoming streams to be infinite.");
        self.node_manager.do_send(NetworkEvent::StreamOpened {
            peer_id: incoming_stream.0,
            stream: Box::new(Stream::new(incoming_stream.1)),
        });
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        println!("finished receiving swarm messages");
    }
}
