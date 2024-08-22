use std::pin::Pin;
use std::task::{Context, Poll};

use futures_util::{Sink as FuturesSink, SinkExt, Stream as FuturesStream};
use libp2p::PeerId;
use tokio::io::BufStream;
use tokio_util::codec::Framed;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

use super::{types, EventLoop};

mod codec;

pub use codec::{CodecError, Message};

pub(crate) const CALIMERO_STREAM_PROTOCOL: libp2p::StreamProtocol =
    libp2p::StreamProtocol::new("/calimero/stream/0.0.1");

#[derive(Debug)]
pub struct Stream {
    inner: Framed<BufStream<Compat<libp2p::Stream>>, codec::MessageJsonCodec>,
}

impl Stream {
    #[must_use]
    pub fn new(stream: libp2p::Stream) -> Self {
        let stream = BufStream::new(stream.compat());
        let stream = Framed::new(stream, codec::MessageJsonCodec::new());
        Self { inner: stream }
    }
}

impl FuturesStream for Stream {
    type Item = Result<Message, CodecError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let inner = Pin::new(&mut self.get_mut().inner);
        inner.poll_next(cx)
    }
}

impl FuturesSink<Message> for Stream {
    type Error = CodecError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready_unpin(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.inner.start_send_unpin(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_flush_unpin(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_close_unpin(cx)
    }
}

impl EventLoop {
    pub(crate) async fn handle_incoming_stream(
        &mut self,
        (peer, stream): (PeerId, libp2p::Stream),
    ) {
        self.event_sender
            .send(types::NetworkEvent::StreamOpened {
                peer_id: peer,
                stream: Box::new(Stream::new(stream)),
            })
            .await
            .expect("Failed to send stream opened event");
    }

    pub(crate) async fn open_stream(&mut self, peer_id: PeerId) -> eyre::Result<Stream> {
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
                eyre::bail!("Failed to open stream: {:?}", err);
            }
        };

        Ok(Stream::new(stream))
    }
}
