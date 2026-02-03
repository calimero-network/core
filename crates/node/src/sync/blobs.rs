use calimero_crypto::{Nonce, SharedKey, NONCE_LEN};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use futures_util::stream::poll_fn;
use futures_util::TryStreamExt;
use rand::{thread_rng, Rng};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;

impl SyncManager {
    pub(super) async fn initiate_blob_share_process(
        &self,
        context: &Context,
        our_identity: PublicKey,
        blob_id: BlobId,
        size: u64,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            our_identity=%our_identity,
            blob_id=%blob_id,
            "Initiating blob share",
        );

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare { blob_id },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let Some(ack) = self.recv(stream, None).await? else {
            bail!("connection closed while awaiting blob share handshake");
        };

        let (their_identity, mut their_nonce) = match ack {
            StreamMessage::Init {
                party_id,
                payload:
                    InitPayload::BlobShare {
                        blob_id: ack_blob_id,
                    },
                next_nonce,
                ..
            } => {
                if ack_blob_id != blob_id {
                    bail!(
                        "unexpected ack blob id: expected {}, got {}",
                        blob_id,
                        ack_blob_id
                    );
                }

                (party_id, next_nonce)
            }
            unexpected @ (StreamMessage::Init { .. }
            | StreamMessage::Message { .. }
            | StreamMessage::OpaqueError) => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        let private_key = self
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.private_key)
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity)?;

        let (tx, mut rx) = mpsc::channel(1);

        let add_task = self.node_client.add_blob(
            poll_fn(|cx| rx.poll_recv(cx)).into_async_read(),
            Some(size),
            None,
        );

        let read_task = async {
            let mut sequencer = Sequencer::default();

            while let Some(msg) = self.recv(stream, Some((shared_key, their_nonce))).await? {
                let (sequence_id, chunk, their_new_nonce) = match msg {
                    StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                    StreamMessage::Message {
                        sequence_id,
                        payload: MessagePayload::BlobShare { chunk },
                        next_nonce,
                    } => (sequence_id, chunk, next_nonce),
                    unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                        bail!("unexpected message: {:?}", unexpected)
                    }
                };

                sequencer.expect(sequence_id)?;

                if chunk.is_empty() {
                    break;
                }

                tx.send(Ok(chunk)).await?;

                their_nonce = their_new_nonce;
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

        info!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            blob_id=%blob_id,
            "Blob share completed",
        );

        Ok(())
    }

    pub(super) async fn handle_blob_share_request(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        blob_id: BlobId,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            blob_id=%blob_id,
            "Received blob share request",
        );

        let Some(mut blob) = self.node_client.get_blob(&blob_id, None).await? else {
            warn!(%blob_id, "blob not found");

            return Ok(());
        };

        let private_key = self
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.private_key)
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity)?;
        let mut our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare { blob_id },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let mut sequencer = Sequencer::default();

        while let Some(chunk) = blob.try_next().await? {
            let our_new_nonce = thread_rng().gen::<Nonce>();
            self.send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sequencer.next(),
                    payload: MessagePayload::BlobShare {
                        chunk: chunk.into_vec().into(),
                    },
                    next_nonce: our_new_nonce,
                },
                Some((shared_key, our_nonce)),
            )
            .await?;

            our_nonce = our_new_nonce;
        }

        self.send(
            stream,
            &StreamMessage::Message {
                sequence_id: sequencer.next(),
                payload: MessagePayload::BlobShare { chunk: b"".into() },
                next_nonce: [0; NONCE_LEN],
            },
            Some((shared_key, our_nonce)),
        )
        .await?;

        info!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            blob_id=%blob_id,
            "Blob share completed",
        );

        Ok(())
    }
}
