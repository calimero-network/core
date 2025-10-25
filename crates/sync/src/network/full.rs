//! Full resync using snapshots for long-offline nodes.

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::Context;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use rand::thread_rng;
use rand::Rng;
use tracing::{debug, info};

use super::manager::{NetworkSyncManager, Sequencer};

impl NetworkSyncManager {
    pub(super) async fn initiate_full_resync_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            our_identity=%our_identity,
            "Initiating full resync",
        );

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::FullSync {
                    application_id: context.application_id,
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        // Wait for acknowledgement and snapshot
        let mut responses = 0;

        while let Some(msg) = self.recv(stream, None).await? {
            let (their_identity, payload, their_nonce) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Init {
                    party_id,
                    payload,
                    next_nonce,
                    ..
                } => (party_id, payload, next_nonce),
                unexpected @ StreamMessage::Message { .. } => {
                    bail!("unexpected message during full resync init: {:?}", unexpected)
                }
            };

            responses += 1;

            if responses > 2 {
                bail!("expected up to two full resync handshakes, got more");
            }

            match payload {
                InitPayload::FullSync {
                    application_id: their_application_id,
                } => {
                    if their_application_id != context.application_id {
                        bail!(
                            "application mismatch: expected {}, got {}",
                            context.application_id,
                            their_application_id
                        );
                    }

                    debug!(
                        context_id=%context.id,
                        our_identity=%our_identity,
                        their_identity=%their_identity,
                        "Received full resync request acknowledgement",
                    );
                }
                unexpected => {
                    bail!("unexpected payload during full resync init: {:?}", unexpected)
                }
            }

            // Get shared key for encrypted transfer
            let private_key = self
                .context_client
                .get_identity(&context.id, &our_identity)?
                .and_then(|i| i.private_key)
                .ok_or_eyre("expected own identity to have private key")?;

            let shared_key = SharedKey::new(&private_key, &their_identity);

            // Receive snapshot
            self.receive_and_apply_snapshot(
                context,
                our_identity,
                their_identity,
                stream,
                shared_key,
                our_nonce,
                their_nonce,
            )
            .await?;

            break;
        }

        if responses == 0 {
            bail!("expected at least one full resync handshake, got none");
        }

        Ok(())
    }

    async fn receive_and_apply_snapshot(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        shared_key: SharedKey,
        mut our_nonce: Nonce,
        mut their_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Receiving snapshot for full resync",
        );

        let mut snapshot_bytes = Vec::new();

        // Receive snapshot in chunks
        while let Some(msg) = self.recv(stream, Some((shared_key, their_nonce))).await? {
            let (artifact, their_new_nonce) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Message {
                    payload: MessagePayload::Snapshot { chunk },
                    next_nonce,
                    ..
                } => (chunk, next_nonce),
                unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                    bail!("unexpected message during snapshot receive: {:?}", unexpected)
                }
            };

            their_nonce = their_new_nonce;

            if artifact.is_empty() {
                // End of snapshot
                break;
            }

            snapshot_bytes.extend_from_slice(&artifact);
        }

        // Deserialize snapshot
        let snapshot = borsh::from_slice::<crate::Snapshot>(&snapshot_bytes)?;

        info!(
            context_id=%context.id,
            entity_count=%snapshot.entity_count,
            index_count=%snapshot.index_count,
            "Received snapshot, applying to storage",
        );

        // Apply snapshot using calimero-sync
        // TODO: Need to create RocksDB storage adaptor
        // For now, use the context client's execute method with a special function
        let outcome = self
            .context_client
            .execute(
                &context.id,
                &our_identity,
                "__calimero_apply_snapshot".to_owned(),
                snapshot_bytes,
                vec![],
                None,
            )
            .await?;

        debug!(
            context_id=%context.id,
            outcome=?outcome,
            "Full resync snapshot applied",
        );

        Ok(())
    }

    pub(super) async fn handle_full_resync_request(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        their_application_id: ApplicationId,
        stream: &mut Stream,
        their_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            our_application_id=%context.application_id,
            their_identity=%their_identity,
            their_application_id=%their_application_id,
            "Received full resync request",
        );

        if their_application_id != context.application_id {
            bail!(
                "application mismatch: expected {}, got {}",
                context.application_id,
                their_application_id
            );
        }

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::FullSync {
                    application_id: context.application_id,
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let private_key = self
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.private_key)
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

        // Generate and send snapshot
        self.generate_and_send_snapshot(
            context,
            our_identity,
            their_identity,
            stream,
            shared_key,
            our_nonce,
            their_nonce,
        )
        .await
    }

    async fn generate_and_send_snapshot(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        shared_key: SharedKey,
        mut our_nonce: Nonce,
        their_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Generating snapshot for full resync",
        );

        // Generate snapshot using context execution
        // This calls into WASM which uses calimero_storage::snapshot
        let outcome = self
            .context_client
            .execute(
                &context.id,
                &our_identity,
                "__calimero_generate_snapshot".to_owned(),
                vec![],
                vec![],
                None,
            )
            .await?;

        let snapshot_bytes = outcome.returns?
            .ok_or_eyre("snapshot generation returned no data")?;

        info!(
            context_id=%context.id,
            snapshot_size=%snapshot_bytes.len(),
            "Generated snapshot, sending to peer",
        );

        // Send snapshot in chunks
        const CHUNK_SIZE: usize = 1024 * 64; // 64KB chunks

        let mut sqx = Sequencer::default();

        for chunk in snapshot_bytes.chunks(CHUNK_SIZE) {
            self.send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx.next(),
                    payload: MessagePayload::Snapshot {
                        chunk: chunk.into(),
                    },
                    next_nonce: our_nonce,
                },
                Some((shared_key, their_nonce)),
            )
            .await?;

            our_nonce = thread_rng().gen();
        }

        // Send empty chunk to signal end
        self.send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx.next(),
                payload: MessagePayload::Snapshot {
                    chunk: (&[] as &[u8]).into(),
                },
                next_nonce: our_nonce,
            },
            Some((shared_key, their_nonce)),
        )
        .await?;

        info!(
            context_id=%context.id,
            "Snapshot sent successfully",
        );

        Ok(())
    }
}

