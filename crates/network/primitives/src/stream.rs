use core::pin::Pin;
use core::task::{Context, Poll};

use futures_util::{Sink as FuturesSink, SinkExt, Stream as FuturesStream, StreamExt};
use libp2p::{Stream as P2pStream, StreamProtocol};
use tokio::io::BufStream;
use tokio_util::codec::Framed;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

mod codec;

use codec::MessageCodec;
pub use codec::{CodecError, Message};

pub const MAX_MESSAGE_SIZE: usize = 8 * 1_024 * 1_024;

pub const CALIMERO_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/calimero/stream/0.0.1");
pub const CALIMERO_BLOB_PROTOCOL: StreamProtocol = StreamProtocol::new("/calimero/blob/0.0.1");

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
