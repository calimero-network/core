use calimero_crypto::SharedKey;
use calimero_network::stream::Stream;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
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
                payload: InitPayload::KeyShare,
            },
            None,
        )
        .await?;

        let Some(ack) = recv(stream, self.sync_config.timeout, None).await? else {
            bail!("connection closed while awaiting state sync handshake");
        };

        let their_identity = match ack {
            StreamMessage::Init {
                party_id,
                payload: InitPayload::KeyShare,
                ..
            } => party_id,
            unexpected @ (StreamMessage::Init { .. }
            | StreamMessage::Message { .. }
            | StreamMessage::OpaqueError) => {
                bail!("unexpected message: {:?}", unexpected)
            }
        };

        self.bidirectional_key_sync(context, our_identity, their_identity, stream)
            .await
    }

    pub(super) async fn handle_key_share_request(
        &self,
        context: Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            their_identity=%their_identity,
            "Received key share request",
        );

        send(
            stream,
            &StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: InitPayload::KeyShare,
            },
            None,
        )
        .await?;

        let mut context = context;
        self.bidirectional_key_sync(&mut context, our_identity, their_identity, stream)
            .await
    }

    async fn bidirectional_key_sync(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_root_hash=%context.root_hash,
            our_identity=%our_identity,
            their_identity=%their_identity,
            "Starting bidirectional key sync",
        );

        let private_key = self
            .ctx_manager
            .get_private_key(context.id, our_identity)?
            .ok_or_eyre("expected own identity to have private key")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

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
            },
            Some(shared_key),
        )
        .await?;

        let Some(msg) = recv(stream, self.sync_config.timeout, Some(shared_key)).await? else {
            bail!("connection closed while awaiting key share");
        };

        let (sequence_id, sender_key) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::KeyShare { sender_key },
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

        Ok(())
    }
}
