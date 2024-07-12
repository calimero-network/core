use futures_util::SinkExt;

use crate::types;

pub(crate) struct CatchupBatchSender {
    batch_size: u8,
    batch: Vec<types::TransactionWithStatus>,
    stream: calimero_network::stream::Stream,
}

impl CatchupBatchSender {
    pub(crate) fn new(batch_size: u8, stream: calimero_network::stream::Stream) -> Self {
        Self {
            batch_size,
            batch: Vec::with_capacity(batch_size as usize),
            stream,
        }
    }

    pub(crate) async fn send(
        &mut self,
        tx_with_status: types::TransactionWithStatus,
    ) -> eyre::Result<()> {
        self.batch.push(tx_with_status);

        if self.batch.len() == self.batch_size as usize {
            let message =
                types::CatchupStreamMessage::TransactionsBatch(types::CatchupTransactionBatch {
                    transactions: std::mem::take(&mut self.batch),
                });

            let message = serde_json::to_vec(&message)?;

            self.stream
                .send(calimero_network::stream::Message { data: message })
                .await?;

            self.batch.clear();
        }

        Ok(())
    }

    pub(crate) async fn flush(&mut self) -> eyre::Result<()> {
        if !self.batch.is_empty() {
            let message =
                types::CatchupStreamMessage::TransactionsBatch(types::CatchupTransactionBatch {
                    transactions: std::mem::take(&mut self.batch),
                });

            let message = serde_json::to_vec(&message)?;

            self.stream
                .send(calimero_network::stream::Message { data: message })
                .await?;
        }

        Ok(())
    }

    pub(crate) async fn flush_with_error(
        &mut self,
        error: types::CatchupError,
    ) -> eyre::Result<()> {
        self.flush().await?;

        let message = serde_json::to_vec(&types::CatchupStreamMessage::Error(error))?;
        self.stream
            .send(calimero_network::stream::Message { data: message })
            .await?;

        Ok(())
    }
}
