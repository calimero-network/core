use calimero_network::stream::Stream;
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use futures_util::stream::poll_fn;
use futures_util::TryStreamExt;
use libp2p::PeerId;
use rand::seq::IteratorRandom;
use rand::thread_rng;
use tokio::sync::mpsc;
use tracing::debug;

use super::{recv, send, Sequencer};
use crate::types::{InitPayload, MessagePayload, StreamMessage};
use crate::Node;

impl Node {
    pub async fn initiate_blob_share_process(
        &self,
        context: &Context,
        blob_id: BlobId,
        size: u64,
        chosen_peer: PeerId,
    ) -> eyre::Result<()> {
        let identities = self.ctx_manager.get_context_owned_identities(context.id)?;

        let Some(our_identity) = identities.into_iter().choose(&mut thread_rng()) else {
            bail!("no identities found for context: {}", context.id);
        };

        let mut stream = self.network_client.open_stream(chosen_peer).await?;

        send(
            &mut stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare { blob_id },
            },
        )
        .await?;

        let Some(ack) = recv(&mut stream, self.sync_config.timeout).await? else {
            bail!("no response to blob share request");
        };

        let _their_identity = match ack {
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
            unexpected => bail!("unexpected message: {:?}", unexpected),
        };

        let (tx, mut rx) = mpsc::channel(1);

        let add_task = self.ctx_manager.add_blob(
            poll_fn(|cx| rx.poll_recv(cx)).into_async_read(),
            Some(size),
            None,
        );

        let read_task = async {
            let mut sequencer = Sequencer::default();

            while let Some(msg) = recv(&mut stream, self.sync_config.timeout).await? {
                let (sequence_id, chunk) = match msg {
                    StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                    StreamMessage::Message {
                        sequence_id,
                        payload: MessagePayload::BlobShare { chunk },
                    } => (sequence_id, chunk),
                    unexpected => bail!("unexpected message: {:?}", unexpected),
                };

                sequencer.test(sequence_id)?;

                tx.send(Ok(chunk)).await?;
            }

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

    pub async fn handle_blob_share_request(
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
            bail!("blob not found: {}", blob_id);
        };

        let identities = self.ctx_manager.get_context_owned_identities(context.id)?;

        let Some(our_identity) = identities.into_iter().choose(&mut thread_rng()) else {
            bail!("no identities found for context: {}", context.id);
        };

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::BlobShare { blob_id },
            },
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
            )
            .await?;
        }

        Ok(())
    }
}
