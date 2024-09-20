use futures_util::AsyncRead;
use futures_util::{ready, Stream as StreamFutures};
use std::pin::Pin;
use std::task::{Context as StdContext, Poll};
use tokio::sync::mpsc;

use crate::types::CatchupApplicationBlobChunk;

pub struct ChunkStream {
    receiver: mpsc::Receiver<CatchupApplicationBlobChunk>,
    buffer: Option<Box<[u8]>>,
    offset: usize,
}

impl ChunkStream {
    pub fn new(receiver: mpsc::Receiver<CatchupApplicationBlobChunk>) -> Self {
        Self {
            receiver,
            buffer: None,
            offset: 0,
        }
    }
}

impl StreamFutures for ChunkStream {
    type Item = CatchupApplicationBlobChunk;

    fn poll_next(self: Pin<&mut Self>, cx: &mut StdContext<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match Pin::new(&mut this.receiver).poll_recv(cx) {
            Poll::Ready(Some(chunk)) => Poll::Ready(Some(chunk)),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
impl AsyncRead for ChunkStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut StdContext<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.buffer.is_none() || self.offset >= self.buffer.as_ref().unwrap().len() {
            match ready!(Pin::new(&mut self).poll_next(cx)) {
                Some(chunk) => {
                    self.buffer = Some(chunk.chunk);
                    self.offset = 0;
                }
                None => return Poll::Ready(Ok(0)),
            }
        }

        if let Some(buffer) = &self.buffer {
            let remaining = &buffer[self.offset..];
            let len = remaining.len().min(buf.len());
            buf[..len].copy_from_slice(&remaining[..len]);
            self.offset += len;
            Poll::Ready(Ok(len))
        } else {
            Poll::Ready(Ok(0))
        }
    }
}
