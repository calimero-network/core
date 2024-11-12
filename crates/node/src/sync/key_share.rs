use calimero_crypto::SharedKey;
use calimero_network::stream::Stream;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use rand::seq::IteratorRandom;
use rand::thread_rng;
use tracing::debug;

use crate::sync::{recv, send, Sequencer};
use crate::types::{InitPayload, MessagePayload, StreamMessage};
use crate::Node;

impl Node {
    pub(super) async fn initiate_key_share_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::KeyShare {},
            },
            None,
        )
        .await?;

        let Some(ack) = recv(stream, self.sync_config.timeout, None).await? else {
            bail!("connection closed while awaiting state sync handshake");
        };

        let sender_key = match ack {
            StreamMessage::Message {
                payload: MessagePayload::KeyShare { sender_key },
                ..
            } => sender_key,
            unexpected @ (StreamMessage::Init { .. }
            | StreamMessage::Message { .. }
            | StreamMessage::OpaqueError) => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        // Do I store "his" SenderKey somewhere?

        Ok(())
    }

    pub(super) async fn handle_key_share_request(
        &self,
        context: Context,
        their_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            "Received key share request",
        );

        let identities = self.ctx_manager.get_context_owned_identities(context.id)?;

        let Some(our_identity) = identities.into_iter().choose(&mut thread_rng()) else {
            bail!("no identities found for context: {}", context.id);
        };

        let sender_key = self
            .ctx_manager
            .get_sender_key(&context.id, &our_identity)?
            .ok_or_eyre("expected own identity to have sender key")?;

        let mut sequencer = Sequencer::default();

        let shared_key = SharedKey::new(&sender_key, &our_identity);

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sequencer.next(),
                payload: MessagePayload::KeyShare { sender_key },
            },
            Some(shared_key), // or None?
        )
        .await?;

        Ok(())
    }
}
