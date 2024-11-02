use std::borrow::Cow;

use calimero_network::stream::Stream;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::Context;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use eyre::bail;
use libp2p::PeerId;
use rand::seq::IteratorRandom;
use rand::thread_rng;
use tracing::debug;

use crate::sync::{recv, send, Sequencer};
use crate::types::{InitPayload, MessagePayload, StreamMessage};
use crate::Node;

impl Node {
    pub async fn initiate_state_sync_process(
        &self,
        context: &mut Context,
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
                payload: InitPayload::StateSync {
                    root_hash: context.root_hash,
                    application_id: context.application_id,
                },
            },
        )
        .await?;

        let Some(ack) = recv(&mut stream, self.sync_config.timeout).await? else {
            bail!("no response to state sync request");
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
            unexpected => bail!("unexpected message: {:?}", unexpected),
        };

        if root_hash == context.root_hash {
            return Ok(());
        }

        let mut sqx_out = Sequencer::default();

        send(
            &mut stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::StateSync {
                    artifact: Cow::from(&[]),
                },
            },
        );

        self.bidirectional_sync(
            context,
            our_identity,
            their_identity,
            &mut sqx_out,
            &mut stream,
        )
        .await?;

        Ok(())
    }

    pub async fn handle_state_sync_request(
        &self,
        context: Context,
        their_identity: PublicKey,
        root_hash: Hash,
        application_id: ApplicationId,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            their_root_hash=%root_hash,
            their_application_id=%application_id,
            "Received state sync request",
        );

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
                    application_id: context.application_id,
                },
            },
        )
        .await?;

        if root_hash == context.root_hash {
            return Ok(());
        }

        let mut sqx_out = Sequencer::default();

        let mut context = context;
        self.bidirectional_sync(
            &mut context,
            our_identity,
            their_identity,
            &mut sqx_out,
            stream,
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
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_root_hash=%context.root_hash,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Starting bidirectional sync",
        );

        let mut sqx_in = Sequencer::default();

        while let Some(msg) = recv(stream, self.sync_config.timeout).await? {
            let (sequence_id, artifact) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::StateSync { artifact },
                } => (sequence_id, artifact),
                unexpected => bail!("unexpected message: {:?}", unexpected),
            };

            sqx_in.test(sequence_id)?;

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

            if outcome.artifact.is_empty() {
                break;
            }

            send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::StateSync {
                        artifact: Cow::from(&outcome.artifact),
                    },
                },
            )
            .await?;
        }

        Ok(())
    }
}
