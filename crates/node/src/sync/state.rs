use std::borrow::Cow;

use calimero_crypto::SharedKey;
use calimero_network::stream::Stream;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::Context;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use tracing::debug;

use crate::sync::{recv, send, Sequencer};
use crate::types::{InitPayload, MessagePayload, StreamMessage};
use crate::Node;

impl Node {
    pub(super) async fn initiate_state_sync_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            our_root_hash=?context.root_hash,
            our_application_id=%context.application_id,
            "Initiating state sync",
        );

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::StateSync {
                    root_hash: context.root_hash,
                    application_id: context.application_id,
                },
            },
            None,
        )
        .await?;

        let mut pair = None;

        for _ in 1..=2 {
            let Some(ack) = recv(stream, self.sync_config.timeout, None).await? else {
                bail!("connection closed while awaiting state sync handshake");
            };

            let (root_hash, their_identity) = match ack {
                StreamMessage::Init {
                    party_id,
                    payload:
                        InitPayload::StateSync {
                            root_hash,
                            application_id,
                        },
                    ..
                } => {
                    if application_id != context.application_id {
                        bail!(
                            "unexpected application id: expected {}, got {}",
                            context.application_id,
                            application_id
                        );
                    }

                    (root_hash, party_id)
                }
                StreamMessage::Init {
                    party_id: their_identity,
                    payload: InitPayload::BlobShare { blob_id },
                    ..
                } => {
                    self.handle_blob_share_request(
                        context,
                        our_identity,
                        their_identity,
                        blob_id,
                        stream,
                    )
                    .await?;

                    continue;
                }
                unexpected @ (StreamMessage::Init { .. }
                | StreamMessage::Message { .. }
                | StreamMessage::OpaqueError) => {
                    bail!("unexpected message: {:?}", unexpected)
                }
            };

            pair = Some((root_hash, their_identity));

            break;
        }

        let Some((root_hash, their_identity)) = pair else {
            bail!("expected two state sync handshakes, got none");
        };

        if root_hash == context.root_hash {
            return Ok(());
        }

        let mut sqx_out = Sequencer::default();

        let private_key = self
            .ctx_manager
            .get_private_key(context.id, our_identity)?
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::StateSync {
                    artifact: b"".into(),
                },
            },
            Some(shared_key),
        )
        .await?;

        self.bidirectional_sync(
            context,
            our_identity,
            their_identity,
            &mut sqx_out,
            stream,
            shared_key,
        )
        .await?;

        Ok(())
    }

    pub(super) async fn handle_state_sync_request(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        their_root_hash: Hash,
        their_application_id: ApplicationId,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            our_root_hash=?context.root_hash,
            our_application_id=%context.application_id,
            their_identity=%their_identity,
            their_root_hash=%their_root_hash,
            their_application_id=%their_application_id,
            "Received state sync request",
        );

        if their_application_id != context.application_id {
            bail!(
                "application mismatch: expected {}, got {}",
                context.application_id,
                their_application_id
            );
        }

        let application = self
            .ctx_manager
            .get_application(&context.application_id)?
            .ok_or_eyre("fatal: the application (even if just a sparse reference) should exist")?;

        if !self.ctx_manager.has_blob_available(application.blob)? {
            debug!(
                context_id=%context.id,
                application_id=%context.application_id,
                "The application blob is not available, attempting to receive it from the other peer",
            );

            self.initiate_blob_share_process(
                &context,
                our_identity,
                application.blob,
                application.size,
                stream,
            )
            .await?;

            debug!(context_id=%context.id, "Resuming state sync");
        }

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::StateSync {
                    root_hash: context.root_hash,
                    application_id: context.application_id,
                },
            },
            None,
        )
        .await?;

        if their_root_hash == context.root_hash {
            return Ok(());
        }

        let private_key = self
            .ctx_manager
            .get_private_key(context.id, our_identity)?
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

        let mut sqx_out = Sequencer::default();

        self.bidirectional_sync(
            context,
            our_identity,
            their_identity,
            &mut sqx_out,
            stream,
            shared_key,
        )
        .await

        // should we compare root hashes again?
    }

    async fn bidirectional_sync(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        sqx_out: &mut Sequencer,
        stream: &mut Stream,
        shared_key: SharedKey,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Starting bidirectional state sync",
        );

        let mut sqx_in = Sequencer::default();

        while let Some(msg) = recv(stream, self.sync_config.timeout, Some(shared_key)).await? {
            let (sequence_id, artifact) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::StateSync { artifact },
                } => (sequence_id, artifact),
                unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                    bail!("unexpected message: {:?}", unexpected)
                }
            };

            sqx_in.test(sequence_id)?;

            if artifact.is_empty() && sqx_out.current() != 0 {
                break;
            }

            let outcome = self
                .execute(
                    context,
                    "__calimero_sync_next",
                    artifact.into_owned(),
                    our_identity,
                )
                .await?
                .ok_or_eyre("the application was not found??")?;

            debug!(
                context_id=%context.id,
                root_hash=?context.root_hash,
                "State sync outcome",
            );

            send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::StateSync {
                        artifact: Cow::from(&outcome.artifact),
                    },
                },
                Some(shared_key),
            )
            .await?;
        }

        debug!(
            context_id=%context.id,
            our_root_hash=%context.root_hash,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "State sync completed",
        );

        Ok(())
    }
}
