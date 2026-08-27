use calimero_crypto::{Nonce, SharedKey, NONCE_LEN};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use futures_util::stream::poll_fn;
use futures_util::{AsyncReadExt, TryStreamExt};
use rand::{thread_rng, Rng};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::manager::SyncManager;
use super::tracking::Sequencer;
use crate::constants::MAX_BLOB_STREAM_SIZE_BYTES;

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
                // Blob bytes are ECDH-encrypted to `our_identity` on the wire,
                // so an impersonator can't read them; no read-gating proof here.
                pop: None,
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
            | StreamMessage::OpaqueError
            | StreamMessage::NotMaterialized) => {
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

        // A ceiling, not an assertion: `size` is the application's recorded size,
        // which is 0 on a stub, so integrity rests on the blob id checked below.
        let size_limit = if size > 0 {
            size.min(MAX_BLOB_STREAM_SIZE_BYTES)
        } else {
            MAX_BLOB_STREAM_SIZE_BYTES
        };
        let add_task = self.node_client.add_blob(
            poll_fn(|cx| rx.poll_recv(cx))
                .into_async_read()
                .take(size_limit),
            None,
            None,
        );

        let read_task = async {
            let mut sequencer = Sequencer::default();
            let mut received = 0_u64;

            while let Some(msg) = self
                .recv(stream, Some((shared_key.clone(), their_nonce)))
                .await?
            {
                let (sequence_id, chunk, their_new_nonce) = match msg {
                    StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                    StreamMessage::Message {
                        sequence_id,
                        payload: MessagePayload::BlobShare { chunk },
                        next_nonce,
                    } => (sequence_id, chunk, next_nonce),
                    unexpected @ (StreamMessage::Init { .. }
                    | StreamMessage::Message { .. }
                    | StreamMessage::NotMaterialized) => {
                        bail!("unexpected message: {:?}", unexpected)
                    }
                };

                sequencer.expect(sequence_id)?;

                if chunk.is_empty() {
                    break;
                }

                // The hard ceiling, applied on the wire rather than through
                // the store, so a rogue sender cannot stream unbounded data.
                received = received.saturating_add(chunk.len() as u64);
                if received > MAX_BLOB_STREAM_SIZE_BYTES {
                    bail!("blob share exceeded {MAX_BLOB_STREAM_SIZE_BYTES} bytes");
                }

                tx.send(Ok(chunk)).await?;

                their_nonce = their_new_nonce;
            }

            drop(tx);

            Ok(())
        };

        let ((received_blob_id, _), _) = tokio::try_join!(add_task, read_task)?;

        if let Err(err) = self
            .node_client
            .verify_stored_blob(received_blob_id, Some(blob_id))
            .await
        {
            warn!(
                %blob_id,
                %received_blob_id,
                advertised_size = size,
                "blob share verification failed",
            );
            return Err(err);
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

        // An http node resolves applications from its registry, so it is not a
        // source of their bytes; to the peer that reads as "not held".
        let held = if self.node_client.may_share_blob(&blob_id)? {
            self.node_client.get_blob(&blob_id, None).await?
        } else {
            None
        };

        let Some(mut blob) = held else {
            warn!(%blob_id, "blob not available to share");
            // Tell the initiator instead of going silent — without a reply it
            // sits in recv() until the stream times out, stalling its whole
            // sync attempt (the upgrade pre-stage path requests blobs this
            // node may not hold yet). OpaqueError makes it bail fast and
            // retry after its backoff.
            if let Err(err) = self.send(stream, &StreamMessage::OpaqueError, None).await {
                warn!(%blob_id, %err, "failed to signal missing blob to initiator");
            }
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
                // Blob bytes are ECDH-encrypted to `our_identity` on the wire,
                // so an impersonator can't read them; no read-gating proof here.
                pop: None,
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
                Some((shared_key.clone(), our_nonce)),
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
