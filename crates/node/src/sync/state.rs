// use core::mem::take;

// use calimero_network::stream::{Message, Stream};
// use eyre::Result as EyreResult;
// use futures_util::SinkExt;
// use serde_json::to_vec as to_json_vec;

// use crate::types::{ActionMessage, BlobError, CatchupActionsBatch, DirectMessage};

// pub struct ActionsBatchSender {
//     batch_size: u8,
//     batch: Vec<ActionMessage>,
//     stream: Box<Stream>,
// }

use std::{cell::RefCell, sync::Arc};

use calimero_network::stream::Stream;
use calimero_primitives::context::Context;
use calimero_primitives::{context::ContextId, hash::Hash, identity::PublicKey};
use eyre::bail;
use libp2p::PeerId;
use rand::seq::IteratorRandom;
use rand::thread_rng;
use tracing::debug;

use crate::sync::{send, Sequencer};
use crate::types::{InitPayload, StreamMessage};
use crate::Node;

impl Node {
    pub async fn initiate_state_sync_process(
        &self,
        context: Context,
        chosen_peer: PeerId,
    ) -> eyre::Result<()> {
        Ok(())
        // let mut stream = self.network_client.open_stream(chosen_peer).await?;

        // let request = CatchupSyncRequest {
        //     context_id: context.id,
        //     root_hash: context.root_hash,
        // };

        // send(&mut stream, DirectMessage::SyncRequest(request))?;

        // let mut sequencer = Sequencer::default();

        // let mut actions_batch_sender = ActionsBatchSender::new(10, stream);

        // for action in context.actions.iter() {
        //     actions_batch_sender.send(action.clone()).await?;
        // }

        // actions_batch_sender.flush().await
    }

    pub async fn handle_state_sync_request(
        &self,
        context: Context,
        their_identity: PublicKey,
        root_hash: Hash,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            their_root_hash=%root_hash,
            "Received state sync request",
        );

        let mut sequencer = Sequencer::default();

        if context.root_hash == root_hash {
            return send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sequencer.next(),
                    payload: None,
                },
            )
            .await;
        }

        let identities = self.ctx_manager.get_context_owned_identities(context.id)?;

        let Some(our_identity) = identities.into_iter().choose(&mut thread_rng()) else {
            bail!("no identities found for context: {}", context.id);
        };

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::StateSync {
                    root_hash: context.root_hash,
                },
            },
        )
        .await?;

        self.bidirectional_sync(
            context,
            our_identity,
            their_identity,
            &mut sequencer,
            stream,
        )
        .await
    }

    async fn bidirectional_sync(
        &self,
        context: Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        sequencer: &mut Sequencer,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        // debug!(
        //     our_root_hash=%context.root_hash,
        //     our_party_id=%party_id,
        //     "Processing state sync request",
        // );

        Ok(())
    }
}

// impl ActionsBatchSender {
//     pub(crate) fn new(batch_size: u8, stream: Box<Stream>) -> Self {
//         Self {
//             batch_size,
//             batch: Vec::with_capacity(batch_size as usize),
//             stream,
//         }
//     }

//     pub(crate) async fn send(&mut self, action_message: ActionMessage) -> EyreResult<()> {
//         self.batch.push(action_message);

//         if self.batch.len() == self.batch_size as usize {
//             let message = DirectMessage::ActionsBatch(CatchupActionsBatch {
//                 actions: take(&mut self.batch),
//             });

//             let message = to_json_vec(&message)?;

//             self.stream.send(Message::new(message)).await?;

//             self.batch.clear();
//         }

//         Ok(())
//     }

//     pub(crate) async fn flush(&mut self) -> EyreResult<()> {
//         if !self.batch.is_empty() {
//             let message = DirectMessage::ActionsBatch(CatchupActionsBatch {
//                 actions: take(&mut self.batch),
//             });

//             let message = to_json_vec(&message)?;

//             self.stream.send(Message::new(message)).await?;
//         }

//         Ok(())
//     }

//     pub(crate) async fn flush_with_error(&mut self, error: BlobError) -> EyreResult<()> {
//         self.flush().await?;

//         let message = to_json_vec(&DirectMessage::Error(error))?;
//         self.stream.send(Message::new(message)).await?;

//         Ok(())
//     }
// }
