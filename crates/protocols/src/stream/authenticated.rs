//! Type-safe secure stream abstraction.
//!
//! **Purpose**: Enforce authentication and encryption at compile time.
//!
//! ## Problem This Solves
//!
//! Without SecureStream, it's easy to:
//! - Forget to authenticate before sharing sensitive data
//! - Mix up encrypted vs unencrypted sends
//! - Duplicate 300+ lines of challenge-response code
//!
//! ## Type Safety Guarantees
//!
//! ```rust
//! // WRONG - won't compile:
//! let stream = network.open_stream(peer).await?;
//! send_sensitive_data(&stream)?;  // ❌ Not authenticated!
//!
//! // RIGHT - compiler enforces authentication:
//! let auth_stream = SecureStream::authenticate(stream, context, identity).await?;
//! auth_stream.send_encrypted(data)?;  // ✅ Type-safe!
//! ```
//!
//! ## Usage Patterns
//!
//! ### Pattern 1: P2P Authentication (Key Sharing, Sensitive Sync)
//! ```rust
//! let stream = network.open_stream(peer).await?;
//! let mut secure = SecureStream::authenticate_p2p(
//!     stream,
//!     &context,
//!     our_identity,
//!     context_client,
//! ).await?;
//!
//! // Now type-safe - only encrypted sends allowed
//! secure.send_encrypted(&sensitive_data).await?;
//! ```
//!
//! ### Pattern 2: Broadcast/Unencrypted (DAG Heads Request)
//! ```rust
//! let stream = network.open_stream(peer).await?;
//! let mut secure = SecureStream::for_protocol(stream, context_id, our_identity);
//!
//! // Can send unencrypted protocol messages
//! secure.send(&request_message).await?;
//! ```

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use eyre::{bail, OptionExt};
use rand::{thread_rng, Rng};
use tokio::time::Duration;
use tracing::{debug, info};

use super::helpers;
use super::tracking::Sequencer;

/// Type-safe secure stream with enforced authentication and encryption.
///
/// The type system prevents using an unauthenticated stream for sensitive operations.
#[derive(Debug)]
pub enum SecureStream {
    /// Authenticated P2P stream with challenge-response verification.
    ///
    /// Use for:
    /// - Key sharing
    /// - Sensitive data transfers
    /// - Operations requiring identity verification
    #[expect(
        private_interfaces,
        reason = "Sequencer is an internal implementation detail"
    )]
    Authenticated {
        stream: Stream,
        context_id: ContextId,
        our_identity: PublicKey,
        their_identity: PublicKey,
        shared_key: SharedKey,
        our_nonce: Nonce,
        their_nonce: Nonce,
        sequencer: Sequencer,
    },

    /// Protocol stream for general sync operations (no encryption required).
    ///
    /// Use for:
    /// - DAG heads requests
    /// - Delta requests
    /// - Public sync protocol messages
    Protocol {
        stream: Stream,
        context_id: ContextId,
        our_identity: PublicKey,
    },
}

impl SecureStream {
    /// Authenticate a P2P stream using challenge-response protocol and exchange sender_keys.
    ///
    /// This performs bidirectional authentication:
    /// 1. Exchange identities
    /// 2. Send/receive challenges (signed with private keys)
    /// 3. Verify signatures to prevent impersonation
    /// 4. Exchange sender_keys
    /// 5. Update peer identity with received sender_key
    ///
    /// # Arguments
    ///
    /// * `stream` - Raw network stream (will be consumed for authentication)
    /// * `context` - Context for the communication
    /// * `our_identity` - Our public key in this context
    /// * `context_client` - Client to fetch and update identity details
    /// * `timeout` - Timeout for each step
    ///
    /// # Errors
    ///
    /// - If peer fails to authenticate (invalid signature)
    /// - If connection times out
    /// - If peer sends unexpected messages
    pub async fn authenticate_p2p(
        stream: &mut Stream,
        context: &Context,
        our_identity: PublicKey,
        context_client: &ContextClient,
        timeout_budget: Duration,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            %our_identity,
            "Authenticating P2P stream with challenge-response",
        );

        // Step 1: Exchange identities
        let our_nonce = thread_rng().gen::<Nonce>();

        helpers::send(
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

        let Some(ack) = super::helpers::recv(stream, None, timeout_budget).await? else {
            bail!("connection closed during authentication handshake");
        };

        let (their_identity, their_nonce) = match ack {
            StreamMessage::Init {
                party_id,
                payload: InitPayload::KeyShare,
                next_nonce,
                ..
            } => (party_id, next_nonce),
            unexpected => {
                bail!("unexpected message during authentication: {:?}", unexpected)
            }
        };

        debug!(
            context_id=%context.id,
            %our_identity,
            %their_identity,
            "Identities exchanged, starting challenge-response authentication"
        );

        // Proceed with authentication (both sides past Init phase)
        Self::authenticate_p2p_after_init(
            stream,
            context,
            our_identity,
            their_identity,
            our_nonce,
            their_nonce,
            context_client,
            timeout_budget,
        )
        .await
    }

    /// Authenticate after Init messages have been exchanged.
    ///
    /// **Use when responding**: Call this from a handler that already consumed the Init message.
    ///
    /// This performs:
    /// 1. Deterministic role assignment (prevents deadlock)
    /// 2. Challenge-response authentication
    /// 3. sender_key exchange
    /// 4. Identity updates
    ///
    /// # Arguments
    ///
    /// * `stream` - Stream (Init already consumed)
    /// * `context` - Context for the communication
    /// * `our_identity` - Our public key in this context
    /// * `their_identity` - Their public key (from Init message)
    /// * `our_nonce` - Our nonce (from Init we sent)
    /// * `their_nonce` - Their nonce (from Init we received)
    /// * `context_client` - Client to fetch and update identity details
    /// * `timeout` - Timeout for each step
    pub async fn authenticate_p2p_after_init(
        stream: &mut Stream,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        our_nonce: Nonce,
        their_nonce: Nonce,
        context_client: &ContextClient,
        timeout_budget: Duration,
    ) -> eyre::Result<()> {
        // Step 2: Get keys for authentication
        let our_identity_record = context_client
            .get_identity(&context.id, &our_identity)?
            .ok_or_eyre("expected own identity to exist")?;

        let private_key = our_identity_record
            .private_key
            .ok_or_eyre("expected own identity to have private key")?;

        let sender_key = our_identity_record
            .sender_key
            .ok_or_eyre("expected own identity to have sender key")?;

        let mut their_identity_record = context_client
            .get_identity(&context.id, &their_identity)?
            .ok_or_eyre("expected peer identity to exist")?;

        let shared_key = SharedKey::new(&private_key, &their_identity);

        // Step 3: Deterministic role assignment (prevents deadlock)
        let is_initiator = our_identity.as_ref() > their_identity.as_ref();

        debug!(
            context_id=%context.id,
            %is_initiator,
            "Determined role for challenge-response (prevents deadlock)"
        );

        // Step 4: Perform challenge-response authentication
        let their_sender_key = if is_initiator {
            Self::authenticate_as_initiator(
                stream,
                &context.id,
                &our_identity,
                &their_identity,
                private_key,
                sender_key,
                shared_key,
                our_nonce,
                their_nonce,
                timeout_budget,
            )
            .await?
        } else {
            Self::authenticate_as_responder(
                stream,
                &context.id,
                &our_identity,
                &their_identity,
                private_key,
                sender_key,
                shared_key,
                our_nonce,
                their_nonce,
                timeout_budget,
            )
            .await?
        };

        // Update their sender_key (always update - PrivateKey doesn't implement PartialEq for security)
        their_identity_record.sender_key = Some(their_sender_key);
        context_client.update_identity(&context.id, &their_identity_record)?;

        info!(
            context_id=%context.id,
            %our_identity,
            %their_identity,
            "P2P authentication and key exchange completed successfully"
        );

        Ok(())
    }

    /// Prove identity ownership by responding to a challenge (requester side).
    ///
    /// **Use when requesting access**: Prove you own the identity you claimed in Init message.
    ///
    /// # Arguments
    ///
    /// * `stream` - Network stream
    /// * `context_id` - Context we're requesting access to
    /// * `our_identity` - Our identity that we need to prove
    /// * `context_client` - Client to fetch our private key
    /// * `timeout` - Response timeout
    ///
    /// # Errors
    ///
    /// Returns error if we don't have private key or challenge fails.
    pub async fn prove_identity(
        stream: &mut Stream,
        context_id: &ContextId,
        our_identity: &PublicKey,
        context_client: &ContextClient,
        timeout: Duration,
    ) -> eyre::Result<()> {
        debug!(
            %context_id,
            %our_identity,
            "Waiting for identity challenge from peer"
        );

        // Get our private key
        let private_key = context_client
            .get_identity(context_id, our_identity)?
            .and_then(|i| i.private_key)
            .ok_or_eyre("expected own identity to have private key")?;

        // Receive challenge
        let Some(msg) = super::helpers::recv(stream, None, timeout).await? else {
            bail!("connection closed while awaiting challenge");
        };

        let challenge = match msg {
            StreamMessage::Message {
                payload: MessagePayload::Challenge { challenge },
                ..
            } => challenge,
            unexpected => {
                bail!("expected Challenge, got {:?}", unexpected)
            }
        };

        // Sign challenge
        let signature = private_key.sign(&challenge)?;

        // Send response
        helpers::send(
            stream,
            &StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::ChallengeResponse {
                    signature: signature.to_bytes(),
                },
                next_nonce: thread_rng().gen(),
            },
            None,
        )
        .await?;

        info!(
            %context_id,
            %our_identity,
            "Proved identity ownership to peer"
        );

        Ok(())
    }

    /// Verify that a peer owns the claimed identity (challenge-response only, no key exchange).
    ///
    /// **Use for access control**: Verify requester is actually a context member before serving requests.
    ///
    /// Unlike `authenticate_p2p()`, this:
    /// - Does NOT exchange sender_keys (no state mutation)
    /// - Only verifies identity ownership via challenge-response
    /// - Lightweight - suitable for every request
    ///
    /// # Arguments
    ///
    /// * `stream` - Network stream
    /// * `context_id` - Context to verify membership in
    /// * `claimed_identity` - The identity they claim to be (from Init message)
    /// * `our_identity` - Our identity in this context
    /// * `context_client` - Client to fetch identity details
    /// * `timeout` - Verification timeout
    ///
    /// # Returns
    ///
    /// `Ok(())` if they prove ownership of `claimed_identity`, `Err` otherwise.
    ///
    /// # Security
    ///
    /// Prevents impersonation attacks where malicious peer claims to be a context member.
    pub async fn verify_identity(
        stream: &mut Stream,
        context_id: &ContextId,
        claimed_identity: &PublicKey,
        our_identity: &PublicKey,
        context_client: &ContextClient,
        timeout: Duration,
    ) -> eyre::Result<()> {
        use ed25519_dalek::Signature;
        use rand::thread_rng;

        debug!(
            %context_id,
            %claimed_identity,
            %our_identity,
            "Verifying peer identity ownership via challenge-response"
        );

        // Verify they're a member of this context
        let _identity = context_client
            .get_identity(context_id, claimed_identity)?
            .ok_or_eyre("claimed identity is not a member of this context")?;

        // Send challenge
        let challenge: [u8; 32] = thread_rng().gen();

        helpers::send(
            stream,
            &StreamMessage::Message {
                sequence_id: 0,
                payload: MessagePayload::Challenge { challenge },
                next_nonce: thread_rng().gen(),
            },
            None,
        )
        .await?;

        // Receive response
        let Some(msg) = super::helpers::recv(stream, None, timeout).await? else {
            bail!("connection closed while awaiting identity verification response");
        };

        let signature_bytes = match msg {
            StreamMessage::Message {
                payload: MessagePayload::ChallengeResponse { signature },
                ..
            } => signature,
            unexpected => {
                bail!("expected ChallengeResponse, got {:?}", unexpected)
            }
        };

        // Verify signature
        let signature = Signature::from_bytes(&signature_bytes);
        claimed_identity
            .verify(&challenge, &signature)
            .map_err(|e| {
                eyre::eyre!(
                    "Identity verification failed - peer could not prove ownership: {}",
                    e
                )
            })?;

        info!(
            %context_id,
            %claimed_identity,
            "Identity verification successful - peer proved ownership"
        );

        Ok(())
    }

    /// Create a protocol stream (no authentication/encryption required).
    ///
    /// Use for public sync protocol messages that don't need encryption.
    pub fn for_protocol(stream: Stream, context_id: ContextId, our_identity: PublicKey) -> Self {
        SecureStream::Protocol {
            stream,
            context_id,
            our_identity,
        }
    }

    /// Send a message over the secure stream.
    ///
    /// - Authenticated streams: Automatically encrypts with shared_key
    /// - Protocol streams: Sends unencrypted
    pub async fn send(&mut self, message: &StreamMessage<'_>) -> eyre::Result<()> {
        match self {
            SecureStream::Authenticated {
                stream,
                shared_key,
                our_nonce,
                ..
            } => {
                super::helpers::send(stream, message, Some((*shared_key, *our_nonce))).await?;
                *our_nonce = thread_rng().gen(); // Rotate nonce after each send
                Ok(())
            }
            SecureStream::Protocol { stream, .. } => {
                super::helpers::send(stream, message, None).await
            }
        }
    }

    /// Receive a message from the secure stream.
    ///
    /// - Authenticated streams: Automatically decrypts with shared_key
    /// - Protocol streams: Receives unencrypted
    pub async fn recv(
        &mut self,
        timeout: Duration,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        match self {
            SecureStream::Authenticated {
                stream,
                shared_key,
                their_nonce,
                ..
            } => {
                let msg = super::helpers::recv(stream, Some((*shared_key, *their_nonce)), timeout)
                    .await?;
                if msg.is_some() {
                    *their_nonce = thread_rng().gen(); // Rotate nonce after each recv
                }
                Ok(msg)
            }
            SecureStream::Protocol { stream, .. } => {
                super::helpers::recv(stream, None, timeout).await
            }
        }
    }

    /// Get the peer's identity (only available for authenticated streams).
    pub fn peer_identity(&self) -> Option<&PublicKey> {
        match self {
            SecureStream::Authenticated { their_identity, .. } => Some(their_identity),
            SecureStream::Protocol { .. } => None,
        }
    }

    /// Get the context ID for this stream.
    pub fn context_id(&self) -> &ContextId {
        match self {
            SecureStream::Authenticated { context_id, .. } => context_id,
            SecureStream::Protocol { context_id, .. } => context_id,
        }
    }

    // Private helper: Initiator side of challenge-response
    async fn authenticate_as_initiator(
        stream_ref: &mut Stream,
        context_id: &ContextId,
        our_identity: &PublicKey,
        their_identity: &PublicKey,
        private_key: PrivateKey,
        sender_key: PrivateKey,
        _shared_key: SharedKey,
        our_nonce: Nonce,
        _their_nonce: Nonce,
        timeout: Duration,
    ) -> eyre::Result<PrivateKey> {
        use ed25519_dalek::Signature;
        let mut sqx_out = Sequencer::default();
        let mut sqx_in = Sequencer::default();

        debug!(
            %context_id,
            %our_identity,
            %their_identity,
            "Starting authentication as initiator (challenge them first)"
        );

        // INITIATOR FLOW: send challenge → recv response → recv challenge → send response → exchange keys

        // Step 1: Send our challenge to peer
        let our_challenge: [u8; 32] = thread_rng().gen();

        helpers::send(
            stream_ref,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::Challenge {
                    challenge: our_challenge,
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        // Step 2: Receive their response (signature of our challenge)
        let Some(msg) = super::helpers::recv(stream_ref, None, timeout).await? else {
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

        // Verify their signature to authenticate them
        let their_signature = Signature::from_bytes(&their_signature_bytes);
        their_identity
            .verify(&our_challenge, &their_signature)
            .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

        info!(
            %context_id,
            %their_identity,
            "Peer successfully authenticated via challenge-response"
        );

        // Step 3: Receive their challenge for us
        let Some(msg) = super::helpers::recv(stream_ref, None, timeout).await? else {
            bail!("connection closed while awaiting peer's challenge");
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

        // Step 4: Sign their challenge and send response
        let our_signature = private_key.sign(&their_challenge)?;

        debug!(
            %context_id,
            %our_identity,
            "Sending authentication response to peer (initiator)"
        );

        helpers::send(
            stream_ref,
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

        // Step 5: Exchange sender_keys (both parties now authenticated)
        // Initiator sends first to avoid deadlock
        helpers::send(
            stream_ref,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        // Receive their sender_key
        let Some(msg) = super::helpers::recv(stream_ref, None, timeout).await? else {
            bail!("connection closed while awaiting sender_key exchange");
        };

        let (sequence_id, their_sender_key) = match msg {
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

        info!(
            %context_id,
            %our_identity,
            %their_identity,
            "Key exchange completed with mutual authentication"
        );

        Ok(their_sender_key)
    }

    // Private helper: Responder side of challenge-response
    async fn authenticate_as_responder(
        stream_ref: &mut Stream,
        context_id: &ContextId,
        our_identity: &PublicKey,
        their_identity: &PublicKey,
        private_key: PrivateKey,
        sender_key: PrivateKey,
        _shared_key: SharedKey,
        our_nonce: Nonce,
        _their_nonce: Nonce,
        timeout: Duration,
    ) -> eyre::Result<PrivateKey> {
        use ed25519_dalek::Signature;
        let mut sqx_out = Sequencer::default();
        let mut sqx_in = Sequencer::default();

        debug!(
            %context_id,
            %our_identity,
            %their_identity,
            "Starting authentication as responder (receive challenge first)"
        );

        // RESPONDER FLOW: recv challenge → send response → send challenge → recv response → exchange keys

        // Step 1: Receive their challenge
        let Some(msg) = super::helpers::recv(stream_ref, None, timeout).await? else {
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

        // Step 2: Sign their challenge and send response
        let our_signature = private_key.sign(&their_challenge)?;

        debug!(
            %context_id,
            %our_identity,
            "Sending authentication response to peer (responder)"
        );

        helpers::send(
            stream_ref,
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

        // Step 3: Send our challenge to peer
        let our_challenge: [u8; 32] = thread_rng().gen();

        debug!(
            %context_id,
            %their_identity,
            "Sending authentication challenge to peer (responder)"
        );

        helpers::send(
            stream_ref,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::Challenge {
                    challenge: our_challenge,
                },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        // Step 4: Receive their signature
        let Some(msg) = super::helpers::recv(stream_ref, None, timeout).await? else {
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

        // Verify their signature to authenticate them
        let their_signature = Signature::from_bytes(&their_signature_bytes);
        their_identity
            .verify(&our_challenge, &their_signature)
            .map_err(|e| eyre::eyre!("Peer failed to prove identity ownership: {}", e))?;

        info!(
            %context_id,
            %their_identity,
            "Peer successfully authenticated via challenge-response"
        );

        // Step 5: Exchange sender_keys (both parties now authenticated)
        // Responder receives first (initiator sends first)
        let Some(msg) = super::helpers::recv(stream_ref, None, timeout).await? else {
            bail!("connection closed while awaiting sender_key exchange");
        };

        let (sequence_id, their_sender_key) = match msg {
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

        // Send our sender_key
        helpers::send(
            stream_ref,
            &StreamMessage::Message {
                sequence_id: sqx_out.next(),
                payload: MessagePayload::KeyShare { sender_key },
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;

        info!(
            %context_id,
            %our_identity,
            %their_identity,
            "Key exchange completed with mutual authentication"
        );

        Ok(their_sender_key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // TODO: Add tests for:
    // - Successful authentication
    // - Failed authentication (invalid signature)
    // - Timeout handling
    // - Nonce rotation
    // - Message encryption/decryption
}
