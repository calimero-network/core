use core::mem::take;

use calimero_network::stream::{Message, Stream};
use eyre::Result as EyreResult;
use futures_util::SinkExt;
use serde_json::to_vec as to_json_vec;

use crate::types::{ActionMessage, CatchupActionsBatch, CatchupError, CatchupStreamMessage};

pub struct ActionsBatchSender {
    batch_size: u8,
    batch: Vec<ActionMessage>,
    stream: Box<Stream>,
}

impl ActionsBatchSender {
    pub(crate) fn new(batch_size: u8, stream: Box<Stream>) -> Self {
        Self {
            batch_size,
            batch: Vec::with_capacity(batch_size as usize),
            stream,
        }
    }

    pub(crate) async fn send(&mut self, action_message: ActionMessage) -> EyreResult<()> {
        self.batch.push(action_message);

        if self.batch.len() == self.batch_size as usize {
            let message = CatchupStreamMessage::ActionsBatch(CatchupActionsBatch {
                actions: take(&mut self.batch),
            });

            let message = to_json_vec(&message)?;

            self.stream.send(Message::new(message)).await?;

            self.batch.clear();
        }

        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> EyreResult<()> {
        if !self.batch.is_empty() {
            let message = CatchupStreamMessage::ActionsBatch(CatchupActionsBatch {
                actions: take(&mut self.batch),
            });

            let message = to_json_vec(&message)?;

            self.stream.send(Message::new(message)).await?;
        }

        Ok(())
    }

    pub(crate) async fn flush_with_error(&mut self, error: CatchupError) -> EyreResult<()> {
        self.flush().await?;

        let message = to_json_vec(&CatchupStreamMessage::Error(error))?;
        self.stream.send(Message::new(message)).await?;

        Ok(())
    }
}
