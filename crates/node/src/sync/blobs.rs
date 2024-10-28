// use core::mem::take;

// use calimero_blobstore::CHUNK_SIZE as BLOB_CHUNK_SIZE;
// use calimero_network::stream::{Message, Stream, MAX_MESSAGE_SIZE as MAX_STREAM_MESSAGE_SIZE};
// use eyre::Result as EyreResult;
// use futures_util::SinkExt;
// use serde_json::to_vec as to_json_vec;

// use crate::types::{CatchupApplicationBlobChunk, DirectMessage};

// pub struct ApplicationBlobChunkSender {
//     batch_size: usize,
//     batch: Vec<u8>,
//     stream: Box<Stream>,
//     sequential_id: u64,
// }

use std::{cell::RefCell, sync::Arc};

use calimero_network::stream::Stream;
use calimero_primitives::{
    application::Application,
    blobs::BlobId,
    context::{Context, ContextId},
    identity::PublicKey,
};
use eyre::bail;
use libp2p::PeerId;
use rand::{seq::IteratorRandom, thread_rng};

use crate::{
    types::{InitPayload, StreamMessage},
    Node,
};

use super::send;

impl Node {
    pub async fn initiate_blob_share_request(
        &self,
        context: &Context,
        application: Application,
        chosen_peer: PeerId,
    ) -> eyre::Result<()> {
        let identities = self.ctx_manager.get_context_owned_identities(context.id)?;

        let Some(our_identity) = identities.into_iter().choose(&mut thread_rng()) else {
            bail!("No identities found for context: {}", context.id);
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        send(
            &mut stream,
            StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare {
                    blob_id: application.blob,
                },
            },
        )
        .await?;

        Ok(())
    }

    pub async fn handle_blob_share(
        &self,
        context_id: ContextId,
        their_identity: PublicKey,
        blob_id: BlobId,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        todo!()
        // let Some(application) = self.ctx_manager.get_application(&blob_id)? else {
        //     bail!("Application not found: {}", blob_id)
        // };

        // let mut blob_sender = ApplicationBlobChunkSender::new(stream);

        // while let Some(chunk) = application.blob.try_next().await? {
        //     blob_sender.send(&chunk).await?;
        // }

        // blob_sender.flush().await
    }
}

// impl ApplicationBlobChunkSender {
//     #[allow(clippy::integer_division, reason = "TODO")]
//     pub(crate) fn new(stream: Box<Stream>) -> Self {
//         // Stream messages are encoded with length delimited codec.
//         // Calculate batch size based on the maximum message size and blob chunk size.
//         // Leave some space for other fields in the message.
//         let batch_size = (MAX_STREAM_MESSAGE_SIZE / BLOB_CHUNK_SIZE) - 1;

//         Self {
//             batch_size,
//             batch: Vec::with_capacity(batch_size * BLOB_CHUNK_SIZE),
//             stream,
//             sequential_id: 0,
//         }
//     }

//     pub(crate) async fn send(&mut self, chunk: &[u8]) -> EyreResult<()> {
//         self.batch.extend_from_slice(&chunk);

//         if self.batch.len() >= self.batch_size * BLOB_CHUNK_SIZE {
//             let message = to_json_vec(&DirectMessage::BlobChunk(CatchupApplicationBlobChunk {
//                 sequential_id: self.sequential_id,
//                 chunk: take(&mut self.batch).into_boxed_slice(),
//             }))?;

//             self.stream.send(Message::new(message)).await?;

//             self.sequential_id += 1;
//         }

//         Ok(())
//     }

//     pub(crate) async fn flush(&mut self) -> EyreResult<()> {
//         if !self.batch.is_empty() {
//             let message = to_json_vec(&DirectMessage::BlobChunk(CatchupApplicationBlobChunk {
//                 sequential_id: self.sequential_id,
//                 chunk: take(&mut self.batch).into_boxed_slice(),
//             }))?;

//             self.stream.send(Message::new(message)).await?;
//         }

//         Ok(())
//     }
// }
