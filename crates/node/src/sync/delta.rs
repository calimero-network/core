use std::num::NonZeroUsize;
use std::pin::pin;

use calimero_context_primitives::ContextAtomic;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::Context;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use futures_util::{StreamExt, TryStreamExt};
use rand::{thread_rng, Rng};
use tracing::debug;

use super::{Sequencer, SyncManager};

impl SyncManager {
    pub(super) async fn initiate_delta_sync_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            our_root_hash=%context.root_hash,
            our_application_id=%context.application_id,
            "Initiating state delta sync",
        );

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::DeltaSync {
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
            let Some(ack) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting delta sync handshake");
            };

            let (their_root_hash, their_identity, their_nonce) = match ack {
                StreamMessage::Init {
                    party_id,
                    payload:
                        InitPayload::DeltaSync {
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

            triple = Some((their_root_hash, their_identity, their_nonce));

            break;
        }

        let Some((their_root_hash, their_identity, their_nonce)) = triple else {
            bail!("expected up to two state delta sync handshakes, got none");
        };

        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            our_root_hash=%context.root_hash,
            their_identity=%their_identity,
            their_root_hash=%their_root_hash,
            "Received state delta sync request acknowledgement",
        );

        if their_root_hash == context.root_hash {
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
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.private_key)
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);
        let our_new_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::DeltaSync {
                    member: [0; 32].into(),
                    height: NonZeroUsize::MIN,
                    delta: None,
                },
                next_nonce: our_new_nonce,
            },
            Some((shared_key, our_nonce)),
        )
        .await?;

        self.bidirectional_delta_sync(
            context,
            our_identity,
            their_identity,
            &mut sqx_out,
            stream,
            shared_key,
            our_new_nonce,
            their_nonce,
        )
        .await
    }

    pub(super) async fn handle_delta_sync_request(
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
            our_root_hash=%context.root_hash,
            our_application_id=%context.application_id,
            their_identity=%their_identity,
            their_root_hash=%their_root_hash,
            their_application_id=%their_application_id,
            "Received state delta sync request",
        );

        if their_application_id != context.application_id {
            bail!(
                "application mismatch: expected {}, got {}",
                context.application_id,
                their_application_id
            );
        }

        let application = self
            .node_client
            .get_application(&context.application_id)?
            .ok_or_eyre("fatal: the application (even if just a sparse reference) should exist")?;

        if !self.node_client.has_blob(&application.blob.bytecode)? {
            debug!(
                context_id=%context.id,
                application_id=%context.application_id,
                "The application blob is not available, attempting to receive it from the other peer",
            );

            self.initiate_blob_share_process(
                &context,
                our_identity,
                application.blob.bytecode,
                application.size,
                stream,
            )
            .await?;

            debug!(context_id=%context.id, "Resuming state delta sync");
        }

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::DeltaSync {
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
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.private_key)
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

        let mut sqx_out = Sequencer::default();

        self.bidirectional_delta_sync(
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
    }

    async fn bidirectional_delta_sync(
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
            "Starting bidirectional state delta sync",
        );

        let members = self.context_client.context_members(&context.id, None);

        let mut members = pin!(members);

        let mut sqx_in = Sequencer::default();

        let mut atomic = ContextAtomic::Lock;

        let mut receiving = true;

        while let Some(msg) = self.recv(stream, Some((shared_key, their_nonce))).await? {
            let (sequence_id, member, height, delta, their_new_nonce) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Message {
                    sequence_id,
                    payload:
                        MessagePayload::DeltaSync {
                            member,
                            height,
                            delta,
                        },
                    next_nonce,
                } => (sequence_id, member, height, delta, next_nonce),
                unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                    bail!("unexpected message: {:?}", unexpected)
                }
            };

            their_nonce = their_new_nonce;

            sqx_in.test(sequence_id)?;

            match delta {
                Some(_) if *member == [0; 32] => receiving = false,
                Some(delta) => {
                    debug!(
                        context_id=%context.id,
                        %member,
                        height,
                        "Received state delta entry",
                    );

                    // todo! what if the height is not sequential?

                    let outcome = self
                        .context_client
                        .execute(
                            &context.id,
                            &our_identity,
                            "__calimero_sync_next".to_owned(),
                            delta.into_owned(),
                            vec![],
                            Some(atomic),
                        )
                        .await?;

                    self.context_client
                        .set_delta_height(&context.id, &member, height)?;

                    atomic = ContextAtomic::Held(
                        outcome
                            .atomic
                            .ok_or_eyre("expected an exclusive lock on the context")?,
                    );

                    context.root_hash = outcome.root_hash;

                    continue;
                }
                _ if *member != [0; 32] => {
                    let deltas =
                        self.context_client
                            .get_state_deltas(&context.id, Some(&member), height);

                    let mut deltas = pin!(deltas);

                    while let Some((_, height, data)) = deltas.try_next().await? {
                        let our_new_nonce = thread_rng().gen::<Nonce>();

                        debug!(
                            context_id=%context.id,
                            %member,
                            height,
                            "Sending state delta entry",
                        );

                        self.send(
                            stream,
                            &StreamMessage::Message {
                                sequence_id: sqx_out.next(),
                                payload: MessagePayload::DeltaSync {
                                    member,
                                    height,
                                    delta: Some(data.into_vec().into()),
                                },
                                next_nonce: our_new_nonce,
                            },
                            Some((shared_key, our_nonce)),
                        )
                        .await?;

                        our_nonce = our_new_nonce;
                    }
                }
                _ => {}
            }

            let Some((member, _)) = members.next().await.transpose()? else {
                if !receiving {
                    break;
                }

                self.send(
                    stream,
                    &StreamMessage::Message {
                        sequence_id: sqx_out.next(),
                        payload: MessagePayload::DeltaSync {
                            member: [0; 32].into(),
                            height: NonZeroUsize::MIN,
                            delta: Some(b"".into()),
                        },
                        next_nonce: [0; 12],
                    },
                    Some((shared_key, our_nonce)),
                )
                .await?;

                continue;
            };

            let height = self
                .context_client
                .get_delta_height(&context.id, &member)?
                .map_or(NonZeroUsize::MIN, |v| v.saturating_add(1));

            let our_new_nonce = thread_rng().gen::<Nonce>();

            debug!(
                context_id=%context.id,
                %member,
                height,
                "Sending state delta query",
            );

            self.send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::DeltaSync {
                        member,
                        height,
                        delta: None,
                    },
                    next_nonce: our_new_nonce,
                },
                Some((shared_key, our_nonce)),
            )
            .await?;

            our_nonce = our_new_nonce;
        }

        debug!(
            context_id=%context.id,
            our_root_hash=%context.root_hash,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Delta sync completed",
        );

        Ok(())
    }
}
