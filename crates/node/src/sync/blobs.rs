use calimero_crypto::SharedKey;
use calimero_network::stream::Stream;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use futures_util::stream::poll_fn;
use futures_util::TryStreamExt;
use rand::seq::IteratorRandom;
use rand::thread_rng;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::{recv, send, Sequencer};
use crate::types::{InitPayload, MessagePayload, StreamMessage};
use crate::Node;

impl Node {
    pub(super) async fn initiate_blob_share_process(
        &self,
        context: &Context,
        our_identity: PublicKey,
        blob_id: BlobId,
        size: u64,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare { blob_id },
            },
            None,
        )
        .await?;

        let Some(ack) = recv(stream, self.sync_config.timeout, None).await? else {
            bail!("connection closed while awaiting blob share handshake");
        };

        let their_identity = match ack {
            StreamMessage::Init {
                party_id,
                payload:
                    InitPayload::BlobShare {
                        blob_id: ack_blob_id,
                    },
                ..
            } => {
                if ack_blob_id != blob_id {
                    bail!(
                        "unexpected ack blob id: expected {}, got {}",
                        blob_id,
                        ack_blob_id
                    );
                }

                party_id
            }
            unexpected @ (StreamMessage::Init { .. }
            | StreamMessage::Message { .. }
            | StreamMessage::OpaqueError) => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        let sender_key = self
            .ctx_manager
            .get_sender_key(&context.id, &our_identity)?
            .ok_or_eyre("expected own identity to have sender key")?;

        let shared_key = SharedKey::new(&sender_key, &their_identity);

        let (tx, mut rx) = mpsc::channel(1);

        let add_task = self.ctx_manager.add_blob(
            poll_fn(|cx| rx.poll_recv(cx)).into_async_read(),
            Some(size),
            None,
        );

        let read_task = async {
            let mut sequencer = Sequencer::default();

            while let Some(msg) = recv(stream, self.sync_config.timeout, Some(shared_key)).await? {
                let (sequence_id, chunk) = match msg {
                    StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                    StreamMessage::Message {
                        sequence_id,
                        payload: MessagePayload::BlobShare { chunk },
                    } => (sequence_id, chunk),
                    unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                        bail!("unexpected message: {:?}", unexpected)
                    }
                };

                sequencer.test(sequence_id)?;

                if chunk.is_empty() {
                    break;
                }

                tx.send(Ok(chunk)).await?;
            }

            drop(tx);

            Ok(())
        };

        let ((received_blob_id, _), _) = tokio::try_join!(add_task, read_task)?;

        if received_blob_id != blob_id {
            bail!(
                "unexpected blob id: expected {}, got {}",
                blob_id,
                received_blob_id
            );
        }

        Ok(())
    }

    pub(super) async fn handle_blob_share_request(
        &self,
        context: Context,
        their_identity: PublicKey,
        blob_id: BlobId,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            blob_id=%blob_id,
            "Received blob share request",
        );

        let Some(mut blob) = self.ctx_manager.get_blob(blob_id)? else {
            warn!(%blob_id, "blob not found");

            return Ok(());
        };

        let identities = self.ctx_manager.get_context_owned_identities(context.id)?;

        let Some(our_identity) = identities.into_iter().choose(&mut thread_rng()) else {
            bail!("no identities found for context: {}", context.id);
        };

        let possible_sending_key = self
            .ctx_manager
            .get_sender_key(&context.id, &our_identity)?;

        let sending_key = match possible_sending_key {
            Some(key) => key,
            None => todo!(),
        };

        let shared_key = SharedKey::new(&sending_key, &our_identity);

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare { blob_id },
            },
            Some(shared_key),
        )
        .await?;

        let mut sequencer = Sequencer::default();

        while let Some(chunk) = blob.try_next().await? {
            send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sequencer.next(),
                    payload: MessagePayload::BlobShare {
                        chunk: chunk.into_vec().into(),
                    },
                },
                Some(shared_key),
            )
            .await?;
        }

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sequencer.next(),
                payload: MessagePayload::BlobShare { chunk: b"".into() },
            },
            Some(shared_key),
        )
        .await?;

        Ok(())
    }
}
