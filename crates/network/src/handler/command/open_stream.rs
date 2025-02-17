use actix::{Context, Handler, Message, ResponseFuture};
use eyre::{bail, Result as EyreResult};
use libp2p::PeerId;

use crate::{
    stream::{Stream, CALIMERO_STREAM_PROTOCOL},
    EventLoop,
};

#[derive(Message, Clone, Copy, Debug)]
#[rtype(result = "EyreResult<Stream>")]
pub struct OpenStream(PeerId);

impl Handler<OpenStream> for EventLoop {
    type Result = ResponseFuture<EyreResult<Stream>>;

    fn handle(
        &mut self,
        OpenStream(peer_id): OpenStream,
        _ctx: &mut Context<Self>,
    ) -> Self::Result {
        let mut stream_control = self.swarm.behaviour().stream.new_control();

        Box::pin(async move {
            let stream = match stream_control
                .open_stream(peer_id, CALIMERO_STREAM_PROTOCOL)
                .await
            {
                Ok(stream) => stream,
                Err(err) => {
                    bail!("Failed to open stream: {:?}", err);
                }
            };

            Ok(Stream::new(stream))
        })
    }
}
