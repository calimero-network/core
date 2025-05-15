use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use rand::{thread_rng, Rng};
use tracing::debug;

use super::{Sequencer, SyncManager};

impl SyncManager {
    pub(super) async fn initiate_key_share_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            "Initiating key share",
        );

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let Some(ack) = self.recv(stream, None).await? else {
            bail!("connection closed while awaiting state sync handshake");
        };

        let (their_identity, their_nonce) = match ack {
            StreamMessage::Init {
                party_id,
                payload: InitPayload::KeyShare,
                next_nonce,
                ..
            } => (party_id, next_nonce),
            unexpected @ (StreamMessage::Init { .. }
            | StreamMessage::Message { .. }
            | StreamMessage::OpaqueError) => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        self.bidirectional_key_share(
            context,
            our_identity,
            their_identity,
            stream,
            our_nonce,
            their_nonce,
        )
        .await
    }

    pub(super) async fn handle_key_share_request(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        their_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            "Received key share request",
        );

        let our_nonce = thread_rng().gen::<Nonce>();

        self.send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        self.bidirectional_key_share(
            context,
            our_identity,
            their_identity,
            stream,
            our_nonce,
            their_nonce,
        )
        .await
    }

    async fn bidirectional_key_share(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        our_nonce: Nonce,
        their_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Starting bidirectional key share",
        );

        let private_key = self
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.sender_key)
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

        let sender_key = self
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| i.sender_key)
            .ok_or_eyre("expected own identity to have sender key")?;

        let mut sqx_out = Sequencer::default();

        self.send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            Some((shared_key, our_nonce)),
        )
        .await?;

        let Some(msg) = self.recv(stream, Some((shared_key, their_nonce))).await? else {
            bail!("connection closed while awaiting key share");
        };

        let (sequence_id, sender_key) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::KeyShare { sender_key },
                ..
            } => (sequence_id, sender_key),
            unexpected @ (StreamMessage::Init { .. }
            | StreamMessage::Message { .. }
            | StreamMessage::OpaqueError) => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        let mut sqx_in = Sequencer::default();

        sqx_in.test(sequence_id)?;

        self.context_client
            .update_sender_key(&context.id, &their_identity, &sender_key)?;

        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Key share completed",
        );

        Ok(())
    }
}
