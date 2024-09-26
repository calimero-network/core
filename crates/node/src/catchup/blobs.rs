use core::mem::take;

use calimero_blobstore::CHUNK_SIZE as BLOB_CHUNK_SIZE;
use calimero_network::stream::{Message, Stream, MAX_MESSAGE_SIZE as MAX_STREAM_MESSAGE_SIZE};
use eyre::Result as EyreResult;
use futures_util::SinkExt;
use serde_json::to_vec as to_json_vec;

use crate::types::{CatchupApplicationBlobChunk, CatchupStreamMessage};

pub struct ApplicationBlobChunkSender {
    batch_size: usize,
    batch: Vec<u8>,
    stream: Box<Stream>,
    sequential_id: u64,
}

impl ApplicationBlobChunkSender {
    pub(crate) fn new(stream: Box<Stream>) -> Self {
        // Stream messages are encoded with length delimited codec.
        // Calculate batch size based on the maximum message size and blob chunk size.
        // Leave some space for other fields in the message.
        let batch_size = (MAX_STREAM_MESSAGE_SIZE / BLOB_CHUNK_SIZE) - 1;

        Self {
            batch_size,
            batch: Vec::with_capacity(batch_size * BLOB_CHUNK_SIZE),
            stream,
            sequential_id: 0,
        }
    }

    pub(crate) async fn send(&mut self, chunk: &[u8]) -> EyreResult<()> {
        self.batch.extend_from_slice(&chunk);

        if self.batch.len() >= self.batch_size * BLOB_CHUNK_SIZE {
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
