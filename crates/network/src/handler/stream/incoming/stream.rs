#[cfg(test)]
#[path = "stream_test.rs"]
mod stream_test;

use core::pin::Pin;
use core::slice;
use core::task::{Context, Poll};
use std::borrow::Cow;
use std::io::Error as IoError;

use bytes::{Bytes, BytesMut};
use futures_util::{Sink as FuturesSink, SinkExt, Stream as FuturesStream, StreamExt};
use libp2p::{Stream as P2pStream, StreamProtocol};
use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;
use tokio::io::BufStream;
use tokio_util::codec::{Decoder, Encoder, Framed, LengthDelimitedCodec};
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Message<'a> {
    pub data: Cow<'a, [u8]>,
}

impl<'a> Message<'a> {
    #[must_use]
    pub fn new<T: Into<Cow<'a, [u8]>>>(data: T) -> Self {
        Self { data: data.into() }
    }
}

#[derive(Debug, ThisError)]
#[non_exhaustive]
pub enum CodecError {
    #[error(transparent)]
    StdIo(#[from] IoError),
}

#[derive(Debug)]
pub struct MessageCodec {
    length_codec: LengthDelimitedCodec,
}

impl MessageCodec {
    pub fn new(max_message_size: usize) -> Self {
        let mut length_codec = LengthDelimitedCodec::new();
        length_codec.set_max_frame_length(max_message_size);
        Self { length_codec }
    }
}

impl Decoder for MessageCodec {
    type Item = Message<'static>;
    type Error = CodecError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let Some(frame) = self.length_codec.decode(src)? else {
            return Ok(None);
        };

        Ok(Some(Message {
            data: Cow::Owned(frame.into()),
        }))
    }
}

impl<'a> Encoder<Message<'a>> for MessageCodec {
    type Error = CodecError;

    fn encode(&mut self, item: Message<'a>, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let data = item.data.as_ref();
        let data = Bytes::from_static(
            // safety: `LengthDelimitedCodec: Encoder` must prepend the length, so it copies `data`
            unsafe { slice::from_raw_parts(data.as_ptr(), data.len()) },
        );
        self.length_codec
            .encode(data, dst)
            .map_err(CodecError::StdIo)
    }
}
