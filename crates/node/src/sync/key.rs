//! Key sharing protocol.
//!
//! **Single Responsibility**: Exchanges cryptographic keys between peers.
//!
//! ## Security Note
//!
//! This protocol relies on libp2p's transport encryption (Noise/TLS) rather than
//! implementing additional application-layer encryption. All streams are already:
//! - Encrypted with ChaCha20-Poly1305 (Noise) or AES-GCM (TLS 1.3)
//! - Authenticated (mutual peer verification)
//! - Forward secret (ephemeral DH keys per connection)
//!
//! See `crates/network/src/behaviour.rs` for transport configuration.

use calimero_crypto::Nonce;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use ed25519_dalek::Signature;
use eyre::{bail, OptionExt};
use rand::{thread_rng, Rng};
use tracing::{debug, info};

use super::manager::SyncManager;
use super::tracking::Sequencer;

impl SyncManager {
    pub(super) async fn initiate_key_share_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        info!(
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

        // Deterministic tie-breaker: use lexicographic comparison to prevent deadlock
        // when both peers initiate simultaneously. Both will agree on who is initiator.
        let is_initiator = our_identity.as_ref() > their_identity.as_ref();

        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            is_initiator=%is_initiator,
            "Determined role via deterministic comparison (prevents deadlock)"
        );

        self.bidirectional_key_share(context, our_identity, their_identity, stream, is_initiator)
            .await
    }

    pub(super) async fn handle_key_share_request(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        _their_nonce: Nonce,
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

        // Use same deterministic tie-breaker as initiate_key_share_process
        // Both peers must agree on roles to prevent deadlock
        let is_initiator = our_identity.as_ref() > their_identity.as_ref();

        debug!(
            context_id=%context.id,
            is_initiator=%is_initiator,
            "Determined role via deterministic comparison (consistent with peer)"
        );

        self.bidirectional_key_share(context, our_identity, their_identity, stream, is_initiator)
            .await
    }

    async fn bidirectional_key_share(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        is_initiator: bool,
    ) -> eyre::Result<()> {
        debug!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity,
            is_initiator=%is_initiator,
            "Starting bidirectional key share with challenge-response authentication",
        );

        let mut their_identity_record = self
            .context_client
            .get_identity(&context.id, &their_identity)?
            .ok_or_eyre("expected peer identity to exist")?;

        let (our_private_key, sender_key) = self
            .context_client
            .get_identity(&context.id, &our_identity)?
            .and_then(|i| Some((i.private_key?, i.sender_key?)))
            .ok_or_eyre("expected own identity to have private & sender keys")?;

        let our_nonce = thread_rng().gen::<Nonce>();
        let mut sqx_out = Sequencer::default();
        let mut sqx_in = Sequencer::default();

        // Asymmetric protocol to avoid deadlock:
        // Initiator: send challenge → recv response → recv challenge → send response → exchange keys
        // Responder: recv challenge → send response → send challenge → recv response → exchange keys

        if is_initiator {
            // INITIATOR: Challenge them first
            let challenge: [u8; 32] = thread_rng().gen();

            debug!(
                context_id=%context.id,
                their_identity=%their_identity,
                "Sending authentication challenge to peer (initiator)"
            );

            self.send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::Challenge { challenge },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;

            // Receive their signature
            let Some(msg) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting challenge response");
            };

            let (sequence_id, their_signature_bytes) = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::ChallengeResponse { signature },
                    ..
                } => (sequence_id, signature),
                unexpected => {
                    bail!("expected ChallengeResponse, got {:?}", unexpected)
                }
            };

            sqx_in.expect(sequence_id)?;

            // Verify their signature
            let their_signature = Signature::from_bytes(&their_signature_bytes);
            their_identity
                .verify(&challenge, &their_signature)
                .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

            info!(
                context_id=%context.id,
                their_identity=%their_identity,
                "Peer successfully authenticated via challenge-response"
            );

            // Now receive their challenge for us
            let Some(msg) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting challenge");
            };

            let (sequence_id, their_challenge) = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::Challenge { challenge },
                    ..
                } => (sequence_id, challenge),
                unexpected => {
                    bail!("expected Challenge, got {:?}", unexpected)
                }
            };

            sqx_in.expect(sequence_id)?;

            // Sign their challenge
            let our_signature = our_private_key.sign(&their_challenge)?;

            debug!(
                context_id=%context.id,
                our_identity=%our_identity,
                "Sending authentication response to peer (initiator)"
            );

            self.send(
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
            // RESPONDER: Receive challenge first, then send ours
            let Some(msg) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting challenge");
            };

            let (sequence_id, their_challenge) = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::Challenge { challenge },
                    ..
                } => (sequence_id, challenge),
                unexpected => {
                    bail!("expected Challenge, got {:?}", unexpected)
                }
            };

            sqx_in.expect(sequence_id)?;

            // Sign their challenge
            let our_signature = our_private_key.sign(&their_challenge)?;

            debug!(
                context_id=%context.id,
                our_identity=%our_identity,
                "Sending authentication response to peer (responder)"
            );

            self.send(
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

            // Now send our challenge
            let challenge: [u8; 32] = thread_rng().gen();

            debug!(
                context_id=%context.id,
                their_identity=%their_identity,
                "Sending authentication challenge to peer (responder)"
            );

            self.send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::Challenge { challenge },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;

            // Receive their signature
            let Some(msg) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting challenge response");
            };

            let (sequence_id, their_signature_bytes) = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::ChallengeResponse { signature },
                    ..
                } => (sequence_id, signature),
                unexpected => {
                    bail!("expected ChallengeResponse, got {:?}", unexpected)
                }
            };

            sqx_in.expect(sequence_id)?;

            // Verify their signature
            let their_signature = Signature::from_bytes(&their_signature_bytes);
            their_identity
                .verify(&challenge, &their_signature)
                .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

            info!(
                context_id=%context.id,
                their_identity=%their_identity,
                "Peer successfully authenticated via challenge-response"
            );
        }

        // Step 6: Now exchange sender_keys (both parties authenticated)
        // Asymmetric to avoid deadlock: initiator sends first, responder sends first
        if is_initiator {
            // Initiator sends their sender_key first
            self.send(
                stream,
                &StreamMessage::Message {
                    sequence_id: sqx_out.next(),
                    payload: MessagePayload::KeyShare { sender_key },
                    next_nonce: our_nonce,
                },
                None,
            )
            .await?;

            // Then receives peer's sender_key
            let Some(msg) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting key share");
            };

            let (sequence_id, peer_sender_key) = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::KeyShare { sender_key },
                    ..
                } => (sequence_id, sender_key),
                unexpected => {
                    bail!("expected KeyShare, got {:?}", unexpected)
                }
            };

            sqx_in.expect(sequence_id)?;
            their_identity_record.sender_key = Some(peer_sender_key);
        } else {
            // Responder receives sender_key first
            let Some(msg) = self.recv(stream, None).await? else {
                bail!("connection closed while awaiting key share");
            };

            let (sequence_id, peer_sender_key) = match msg {
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::KeyShare { sender_key },
                    ..
                } => (sequence_id, sender_key),
                unexpected => {
                    bail!("expected KeyShare, got {:?}", unexpected)
                }
            };

            sqx_in.expect(sequence_id)?;
            their_identity_record.sender_key = Some(peer_sender_key);

            // Then sends their sender_key
            self.send(
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

        // Update their identity with received sender_key (already set in branches above)
        self.context_client
            .update_identity(&context.id, &their_identity_record)?;

        info!(
            context_id=%context.id,
            our_identity=%our_identity,
            their_identity=%their_identity_record.public_key,
            "Key share completed with mutual authentication",
        );

        Ok(())
    }
}
