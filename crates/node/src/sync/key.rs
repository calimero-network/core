use calimero_crypto::{Nonce, SharedKey};
use calimero_network::stream::Stream;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use rand::{thread_rng, Rng};
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
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            "Initiating key share",
        );

        let nonce = thread_rng().gen::<Nonce>();

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
                nonce,
            },
            None,
        )
        .await?;

        let Some(ack) = recv(stream, self.sync_config.timeout, None).await? else {
            bail!("connection closed while awaiting state sync handshake");
        };

        let (their_identity, their_nonce) = match ack {
            StreamMessage::Init {
                party_id,
                payload: InitPayload::KeyShare,
                nonce,
                ..
            } => (party_id, nonce),
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
            nonce,
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
        nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            "Received key share request",
        );

        let their_nonce = nonce;

        let nonce = thread_rng().gen::<Nonce>();

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
                nonce: nonce,
            },
            None,
        )
        .await?;

        self.bidirectional_key_share(
            context,
            our_identity,
            their_identity,
            stream,
            nonce,
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
        sending_nonce: Nonce,
        receiving_nonce: Nonce,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Starting bidirectional key share",
        );

        let private_key = self
            .ctx_manager
            .get_private_key(context.id, our_identity)?
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);
        let new_nonce = thread_rng().gen::<Nonce>();

        let sender_key = self
            .ctx_manager
            .get_sender_key(&context.id, &our_identity)?
            .ok_or_eyre("expected own identity to have sender key")?;

        let mut sqx_out = Sequencer::default();

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::KeyShare { sender_key },
                nonce: new_nonce,
            },
            Some((shared_key, sending_nonce)),
        )
        .await?;

        let Some(msg) = recv(
            stream,
            self.sync_config.timeout,
            Some((shared_key, receiving_nonce)),
        )
        .await?
        else {
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

        self.ctx_manager
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
