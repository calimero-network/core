use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::messages::OpenStream;
use calimero_network_primitives::stream::{Stream, CALIMERO_STREAM_PROTOCOL};
use eyre::bail;

use crate::NetworkManager;

impl Handler<OpenStream> for NetworkManager {
    type Result = ResponseFuture<<OpenStream as Message>::Result>;

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
