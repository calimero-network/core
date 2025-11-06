use std::time::Duration;

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::direct::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use ed25519_dalek::Signature;
use eyre::{bail, OptionExt, Result};
use rand::{thread_rng, Rng};
use tracing::{debug, info};

use crate::sync::stream::{recv, send};
use crate::sync::Sequencer;

const MESSAGE_BUDGET_DIVISOR: u32 = 3;

fn budget(timeout: Duration) -> Duration {
    timeout / MESSAGE_BUDGET_DIVISOR
}

pub(crate) async fn initiate_key_share_process(
    context_client: &ContextClient,
    context: &mut Context,
    our_identity: PublicKey,
    stream: &mut Stream,
    timeout: Duration,
) -> Result<()> {
    info!(
        context_id=%context.id,
        our_identity=%our_identity,
        "Initiating key share",
    );

    let our_nonce = thread_rng().gen::<Nonce>();

    send(
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

    let Some(ack) = recv(stream, None, budget(timeout)).await? else {
        bail!("connection closed while awaiting state sync handshake");
    };

    let their_identity = match ack {
        StreamMessage::Init {
            party_id,
            payload: InitPayload::KeyShare,
            ..
        } => party_id,
        unexpected => bail!("unexpected message: {:?}", unexpected),
    };

    let is_initiator = our_identity.as_ref() > their_identity.as_ref();

    debug!(
        context_id=%context.id,
        is_initiator=%is_initiator,
        "Determined role via deterministic comparison (consistent with peer)"
    );

    bidirectional_key_share(
        context_client,
        context,
        our_identity,
        their_identity,
        stream,
        is_initiator,
        our_nonce,
        timeout,
    )
    .await
}

pub(crate) async fn handle_key_share_request(
    context_client: &ContextClient,
    context: &Context,
    our_identity: PublicKey,
    their_identity: PublicKey,
    stream: &mut Stream,
    timeout: Duration,
) -> Result<()> {
    debug!(
        context_id=%context.id,
        their_identity=%their_identity,
        "Received key share request",
    );

    let our_nonce = thread_rng().gen::<Nonce>();

    send(
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

    let is_initiator = our_identity.as_ref() > their_identity.as_ref();

    debug!(
        context_id=%context.id,
        is_initiator=%is_initiator,
        "Determined role via deterministic comparison (consistent with peer)"
    );

    bidirectional_key_share(
        context_client,
        context,
        our_identity,
        their_identity,
        stream,
        is_initiator,
        our_nonce,
        timeout,
    )
    .await
}

async fn bidirectional_key_share(
    context_client: &ContextClient,
    context: &Context,
    our_identity: PublicKey,
    their_identity: PublicKey,
    stream: &mut Stream,
    is_initiator: bool,
    our_nonce: Nonce,
    timeout: Duration,
) -> Result<()> {
    debug!(
        context_id=%context.id,
        our_identity=%our_identity,
        their_identity=%their_identity,
        is_initiator=%is_initiator,
        "Starting bidirectional key share with challenge-response authentication",
    );

    let mut their_identity_record = context_client
        .get_identity(&context.id, &their_identity)?
        .ok_or_eyre("expected peer identity to exist")?;

    let (our_private_key, sender_key) = context_client
        .get_identity(&context.id, &our_identity)?
        .and_then(|i| Some((i.private_key?, i.sender_key?)))
        .ok_or_eyre("expected own identity to have private & sender keys")?;

    let mut sqx_out = Sequencer::default();
    let mut sqx_in = Sequencer::default();

    if is_initiator {
        let challenge: [u8; 32] = thread_rng().gen();

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::Challenge { challenge },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let Some(msg) = recv(stream, None, budget(timeout)).await? else {
            bail!("connection closed while awaiting challenge response");
        };

        let (sequence_id, their_signature_bytes) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::ChallengeResponse { signature },
                ..
            } => (sequence_id, signature),
            unexpected => bail!("expected ChallengeResponse, got {:?}", unexpected),
        };

        sqx_in.expect(sequence_id)?;

        let their_signature = Signature::from_bytes(&their_signature_bytes);
        their_identity_record
            .public_key
            .verify(&challenge, &their_signature)
            .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

        info!(
            context_id=%context.id,
            their_identity=%their_identity,
            "Peer successfully authenticated via challenge-response"
        );

        let Some(msg) = recv(stream, None, budget(timeout)).await? else {
            bail!("connection closed while awaiting challenge");
        };

        let (sequence_id, their_challenge) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::Challenge { challenge },
                ..
            } => (sequence_id, challenge),
            unexpected => bail!("expected Challenge, got {:?}", unexpected),
        };

        sqx_in.expect(sequence_id)?;

        let our_signature = our_private_key.sign(&their_challenge)?;

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::ChallengeResponse {
                    signature: our_signature.to_bytes(),
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;
    } else {
        let Some(msg) = recv(stream, None, budget(timeout)).await? else {
            bail!("connection closed while awaiting challenge");
        };

        let (sequence_id, their_challenge) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::Challenge { challenge },
                ..
            } => (sequence_id, challenge),
            unexpected => bail!("expected Challenge, got {:?}", unexpected),
        };

        sqx_in.expect(sequence_id)?;

        let our_signature = our_private_key.sign(&their_challenge)?;

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::ChallengeResponse {
                    signature: our_signature.to_bytes(),
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let challenge: [u8; 32] = thread_rng().gen();

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::Challenge { challenge },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let Some(msg) = recv(stream, None, budget(timeout)).await? else {
            bail!("connection closed while awaiting challenge response");
        };

        let (sequence_id, their_signature_bytes) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::ChallengeResponse { signature },
                ..
            } => (sequence_id, signature),
            unexpected => bail!("expected ChallengeResponse, got {:?}", unexpected),
        };

        sqx_in.expect(sequence_id)?;

        let their_signature = Signature::from_bytes(&their_signature_bytes);
        their_identity_record
            .public_key
            .verify(&challenge, &their_signature)
            .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

        info!(
            context_id=%context.id,
            their_identity=%their_identity,
            "Peer successfully authenticated via challenge-response"
        );
    }

    if is_initiator {
        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        let Some(msg) = recv(stream, None, budget(timeout)).await? else {
            bail!("connection closed while awaiting key share");
        };

        let (sequence_id, peer_sender_key) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::KeyShare { sender_key },
                ..
            } => (sequence_id, sender_key),
            unexpected => bail!("expected KeyShare, got {:?}", unexpected),
        };

        sqx_in.expect(sequence_id)?;
        their_identity_record.sender_key = Some(peer_sender_key);
    } else {
        let Some(msg) = recv(stream, None, budget(timeout)).await? else {
            bail!("connection closed while awaiting key share");
        };

        let (sequence_id, peer_sender_key) = match msg {
            StreamMessage::Message {
                sequence_id,
                payload: MessagePayload::KeyShare { sender_key },
                ..
            } => (sequence_id, sender_key),
            unexpected => bail!("expected KeyShare, got {:?}", unexpected),
        };

        sqx_in.expect(sequence_id)?;
        their_identity_record.sender_key = Some(peer_sender_key);

        send(
            stream,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;
    }

    context_client.update_identity(&context.id, &their_identity_record)?;

    info!(
        context_id=%context.id,
        our_identity=%our_identity,
        their_identity=%their_identity_record.public_key,
        "Key share completed with mutual authentication",
    );

    Ok(())
}
