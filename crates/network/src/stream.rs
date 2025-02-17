#![allow(
    clippy::allow_attributes,
    reason = "Needed for lints that don't follow expect"
)]

use core::pin::Pin;
use core::task::{Context, Poll};

use eyre::{bail, Result as EyreResult};
use futures_util::{Sink as FuturesSink, SinkExt, Stream as FuturesStream, StreamExt};
use libp2p::{PeerId, Stream as P2pStream, StreamProtocol};
use tokio::io::BufStream;
use tokio_util::codec::Framed;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

use super::EventLoop;
use crate::stream::codec::MessageCodec;

mod codec;

pub use codec::{CodecError, Message};

pub const MAX_MESSAGE_SIZE: usize = 8 * 1_024 * 1_024;

pub(crate) const CALIMERO_STREAM_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/calimero/stream/0.0.1");

#[derive(Debug)]
pub struct Stream {
    inner: Framed<BufStream<Compat<P2pStream>>, MessageCodec>,
}

impl Stream {
    #[must_use]
    pub fn new(stream: P2pStream) -> Self {
        let stream = BufStream::new(stream.compat());
        let stream = Framed::new(stream, MessageCodec::new(MAX_MESSAGE_SIZE));
        Self { inner: stream }
    }
}

impl FuturesStream for Stream {
    type Item = Result<Message<'static>, CodecError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx)
    }
}

impl<'a> FuturesSink<Message<'a>> for Stream {
    type Error = CodecError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message<'a>) -> Result<(), Self::Error> {
        self.inner.start_send_unpin(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx)
    }
}

// TODO: The &mut self usages are needed for reasons not yet apparent, despite
// TODO: not actually making any self-modifications. If removed, they cause
// TODO: errors about Send compatibility.
#[expect(
    clippy::needless_pass_by_ref_mut,
    reason = "TODO: This should be refactored"
)]
#[allow(
    clippy::multiple_inherent_impl,
    reason = "Currently needed due to code structure"
)]
impl EventLoop {
    pub(crate) fn handle_incoming_stream(&mut self, (peer, stream): (PeerId, P2pStream)) {
        // self.event_sender
        //     .send(NetworkEvent::StreamOpened {
        //         peer_id: peer,
        //         stream: Box::new(Stream::new(stream)),
        //     })
        //     .await
        //     .expect("Failed to send stream opened event");
    }

    pub(crate) async fn open_stream(&mut self, peer_id: PeerId) -> EyreResult<Stream> {
        let stream = match self
            .swarm
            .behaviour()
            .stream
            .new_control()
            .open_stream(peer_id, CALIMERO_STREAM_PROTOCOL)
            .await
        {
            Ok(stream) => stream,
            Err(err) => {
                bail!("Failed to open stream: {:?}", err);
            }
        };

        Ok(Stream::new(stream))
    }
}
