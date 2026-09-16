use core::pin::Pin;
use core::task::{Context, Poll};

use futures_util::{Sink as FuturesSink, SinkExt, Stream as FuturesStream, StreamExt};
use libp2p::{Stream as P2pStream, StreamProtocol};
use tokio::io::BufStream;
#[cfg(feature = "test-utils")]
use tokio::io::{duplex, DuplexStream};
use tokio_util::codec::Framed;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

mod codec;

use codec::MessageCodec;
pub use codec::{CodecError, Message};

pub const MAX_MESSAGE_SIZE: usize = 8 * 1_024 * 1_024;

pub const CALIMERO_STREAM_PROTOCOL: StreamProtocol = StreamProtocol::new("/calimero/stream/0.0.2");
pub const CALIMERO_BLOB_PROTOCOL: StreamProtocol = StreamProtocol::new("/calimero/blob/0.0.2");

/// "I now hold this blob for this context" — a one-shot, one-message notice
/// sent to a context's availability nodes so they can prefetch it.
///
/// A protocol of its own rather than a new message kind on
/// [`CALIMERO_BLOB_PROTOCOL`]: that protocol's server parses the first frame
/// strictly as a `BlobRequest`, so widening it would mean a version bump to
/// `/calimero/blob/0.0.3` and a lock-step upgrade of the whole transfer path.
/// A separate protocol leaves transfer untouched and simply is not negotiated
/// by peers that do not speak it.
pub const CALIMERO_BLOB_ANNOUNCE_PROTOCOL: StreamProtocol =
    StreamProtocol::new("/calimero/blob-announce/1.0.0");

type Libp2pFramed = Framed<BufStream<Compat<P2pStream>>, MessageCodec>;

#[cfg(feature = "test-utils")]
type MemoryFramed = Framed<BufStream<DuplexStream>, MessageCodec>;

#[derive(Debug)]
pub struct Stream {
    inner: StreamInner,
}

/// Backing transport for a [`Stream`].
///
/// Production always uses [`StreamInner::Libp2p`]; the in-memory
/// variant exists only so the sync mocks can hand back a genuine
/// `Ok(Stream)` (not just `Err`) without standing up a libp2p swarm.
/// It is compiled away entirely unless the `test-utils` feature is on,
/// so the `match` in the poll impls collapses to a single arm in
/// production builds.
#[derive(Debug)]
enum StreamInner {
    Libp2p(Libp2pFramed),
    #[cfg(feature = "test-utils")]
    Memory(MemoryFramed),
}

impl Stream {
    #[must_use]
    pub fn new(stream: P2pStream) -> Self {
        let stream = BufStream::new(stream.compat());
        let stream = Framed::new(stream, MessageCodec::new(MAX_MESSAGE_SIZE));
        Self {
            inner: StreamInner::Libp2p(stream),
        }
    }

    /// Construct a connected in-memory `Stream` pair for tests.
    ///
    /// The two ends are wired by a `tokio::io::duplex` pipe and carry
    /// the same [`MessageCodec`] framing as a real libp2p stream, so
    /// code under test can both observe a successful open (`Ok(Stream)`)
    /// and exchange messages end-to-end without a libp2p swarm. Gated
    /// behind the `test-utils` feature so production binaries never
    /// compile the in-memory transport.
    #[cfg(feature = "test-utils")]
    #[must_use]
    pub fn test_pair() -> (Self, Self) {
        let (a, b) = duplex(MAX_MESSAGE_SIZE);
        (Self::from_duplex(a), Self::from_duplex(b))
    }

    #[cfg(feature = "test-utils")]
    fn from_duplex(half: DuplexStream) -> Self {
        let stream = BufStream::new(half);
        let stream = Framed::new(stream, MessageCodec::new(MAX_MESSAGE_SIZE));
        Self {
            inner: StreamInner::Memory(stream),
        }
    }
}

impl FuturesStream for Stream {
    type Item = Result<Message<'static>, CodecError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.inner {
            StreamInner::Libp2p(inner) => inner.poll_next_unpin(cx),
            #[cfg(feature = "test-utils")]
            StreamInner::Memory(inner) => inner.poll_next_unpin(cx),
        }
    }
}

impl<'a> FuturesSink<Message<'a>> for Stream {
    type Error = CodecError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.inner {
            StreamInner::Libp2p(inner) => inner.poll_ready_unpin(cx),
            #[cfg(feature = "test-utils")]
            StreamInner::Memory(inner) => inner.poll_ready_unpin(cx),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message<'a>) -> Result<(), Self::Error> {
        match &mut self.inner {
            StreamInner::Libp2p(inner) => inner.start_send_unpin(item),
            #[cfg(feature = "test-utils")]
            StreamInner::Memory(inner) => inner.start_send_unpin(item),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.inner {
            StreamInner::Libp2p(inner) => inner.poll_flush_unpin(cx),
            #[cfg(feature = "test-utils")]
            StreamInner::Memory(inner) => inner.poll_flush_unpin(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.inner {
            StreamInner::Libp2p(inner) => inner.poll_close_unpin(cx),
            #[cfg(feature = "test-utils")]
            StreamInner::Memory(inner) => inner.poll_close_unpin(cx),
        }
    }
}
