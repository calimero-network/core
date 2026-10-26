use actix::{Context, Handler, Message, ResponseFuture};
use calimero_network_primitives::messages::{DialFailed, OpenStream};
use calimero_network_primitives::stream::{Stream, CALIMERO_STREAM_PROTOCOL};
use libp2p_stream::OpenStreamError;

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
                Err(err) => return Err(open_stream_error(err)),
            };

            Ok(Stream::new(stream))
        })
    }
}

/// `NotConnected` is the only error libp2p-stream raises for a failed dial, and the
/// only form it leaves of the swarm's `DialError`.
pub(crate) fn open_stream_error(err: OpenStreamError) -> eyre::Report {
    match err {
        OpenStreamError::Io(err) if err.kind() == std::io::ErrorKind::NotConnected => {
            DialFailed(err).into()
        }
        err => eyre::eyre!("Failed to open stream: {err:?}"),
    }
}
