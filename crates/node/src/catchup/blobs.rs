use core::mem::take;
use std::pin::Pin;
use std::task::{Context as StdContext, Poll};

use calimero_blobstore::CHUNK_SIZE as BLOB_CHUNK_SIZE;
use calimero_network::stream::{Message, Stream};
use eyre::Result as EyreResult;
use futures_util::{AsyncRead, SinkExt, Stream as StreamFutures};
use serde_json::to_vec as to_json_vec;
use tokio::sync::mpsc;

use crate::types::{CatchupApplicationBlobChunk, CatchupStreamMessage};

pub struct ApplicationBlobChunkStream {
    receiver: mpsc::Receiver<CatchupApplicationBlobChunk>,
}

impl ApplicationBlobChunkStream {
    pub fn new(receiver: mpsc::Receiver<CatchupApplicationBlobChunk>) -> Self {
        Self { receiver }
    }
}

impl StreamFutures for ApplicationBlobChunkStream {
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

impl AsyncRead for ApplicationBlobChunkStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut StdContext<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();

        match Pin::new(&mut this.receiver).poll_recv(cx) {
            Poll::Ready(Some(chunk)) => {
                let data = &chunk.chunk;
                let len = data.len().min(buf.len());
                buf[..len].copy_from_slice(&data[..len]);
                Poll::Ready(Ok(len))
            }
            Poll::Ready(None) => Poll::Ready(Ok(0)),
            Poll::Pending => Poll::Pending,
        }
    }
}

pub struct ApplicationBlobChunkSender {
    batch_size: u8,
    batch: Vec<u8>,
    stream: Box<Stream>,
    sequential_id: u64,
}

impl ApplicationBlobChunkSender {
    pub(crate) fn new(batch_size: u8, stream: Box<Stream>) -> Self {
        Self {
            batch_size,
            batch: Vec::with_capacity((batch_size as usize) * BLOB_CHUNK_SIZE),
            stream,
            sequential_id: 0,
        }
    }

    pub(crate) async fn send(&mut self, chunk: &[u8]) -> EyreResult<()> {
        self.batch.extend_from_slice(&chunk);

        if self.batch.len() >= (self.batch_size as usize) * BLOB_CHUNK_SIZE {
            let message = to_json_vec(&CatchupStreamMessage::ApplicationBlobChunk(
                CatchupApplicationBlobChunk {
                    sequential_id: self.sequential_id,
                    chunk: take(&mut self.batch).into_boxed_slice(),
                },
            ))?;

            self.stream.send(Message::new(message)).await?;

            self.sequential_id += 1;
        }

        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> EyreResult<()> {
        if !self.batch.is_empty() {
            let message = to_json_vec(&CatchupStreamMessage::ApplicationBlobChunk(
                CatchupApplicationBlobChunk {
                    sequential_id: self.sequential_id,
                    chunk: take(&mut self.batch).into_boxed_slice(),
                },
            ))?;

            self.stream.send(Message::new(message)).await?;
        }

        Ok(())
    }
}
