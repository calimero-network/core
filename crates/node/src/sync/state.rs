use std::borrow::Cow;

use calimero_crypto::{Nonce, SharedKey};
use calimero_network::stream::Stream;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::Context;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use rand::{thread_rng, Rng};
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

        let our_nonce = thread_rng().gen::<Nonce>();

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::StateSync {
                    root_hash: context.root_hash,
                    application_id: context.application_id,
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let mut triple = None;

        for _ in 1..=2 {
            let Some(ack) = recv(stream, self.sync_config.timeout, None).await? else {
                bail!("connection closed while awaiting state sync handshake");
            };

            let (root_hash, their_identity, their_nonce) = match ack {
                StreamMessage::Init {
                    party_id,
                    payload:
                        InitPayload::StateSync {
                            root_hash,
                            application_id,
                        },
                    next_nonce,
                    ..
                } => {
                    if application_id != context.application_id {
                        bail!(
                            "unexpected application id: expected {}, got {}",
                            context.application_id,
                            application_id
                        );
                    }

                    (root_hash, party_id, next_nonce)
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

            triple = Some((root_hash, their_identity, their_nonce));

            break;
        }

        let Some((root_hash, their_identity, their_nonce)) = triple else {
            bail!("expected two state sync handshakes, got none");
        };

        if root_hash == context.root_hash {
            debug!(
                context_id=%context.id,
                our_identity=%our_identity,
                their_identity=%their_identity,
                "Root hashes match, up to date",
            );

            return Ok(());
        }

        let mut sqx_out = Sequencer::default();

        let private_key = self
            .ctx_manager
            .get_private_key(context.id, our_identity)?
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);
        let our_new_nonce = thread_rng().gen::<Nonce>();

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::StateSync {
                    artifact: b"".into(),
                },
                next_nonce: our_new_nonce,
            },
            Some((shared_key, our_nonce)),
        )
        .await?;
        self.bidirectional_sync(
            context,
            our_identity,
            their_identity,
            &mut sqx_out,
            stream,
            shared_key,
            our_new_nonce,
            their_nonce,
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
        their_nonce: Nonce,
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

        let our_nonce = thread_rng().gen::<Nonce>();

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::StateSync {
                    root_hash: context.root_hash,
                    application_id: context.application_id,
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        if their_root_hash == context.root_hash {
            debug!(
                context_id=%context.id,
                our_identity=%our_identity,
                their_identity=%their_identity,
                "Root hashes match, up to date",
            );

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
            our_nonce,
            their_nonce,
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
        mut our_nonce: Nonce,
        mut their_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Starting bidirectional state sync",
        );

        let mut sqx_in = Sequencer::default();

        while let Some(msg) = recv(
            stream,
            self.sync_config.timeout,
            Some((shared_key, their_nonce)),
        )
        .await?
        {
            let (sequence_id, artifact, their_new_nonce) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::StateSync { artifact },
                    next_nonce,
                } => (sequence_id, artifact, next_nonce),
                unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                    bail!("unexpected message: {:?}", unexpected)
                }
            };

            their_nonce = their_new_nonce;

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

            let our_new_nonce = (!outcome.artifact.is_empty())
                .then(|| thread_rng().gen())
                .unwrap_or_default();

            send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::StateSync {
                        artifact: Cow::from(&outcome.artifact),
                    },
                    next_nonce: our_new_nonce,
                },
                Some((shared_key, our_nonce)),
            )
            .await?;

            if our_new_nonce == [0; 12] {
                break;
            }

            our_nonce = our_new_nonce;
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
