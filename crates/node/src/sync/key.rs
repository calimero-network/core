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

use calimero_network_primitives::stream::Stream;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use rand::{thread_rng, Rng};
use tracing::info;

use super::manager::SyncManager;
use super::secure_stream::SecureStream;

impl SyncManager {
    /// Initiate key sharing with a peer.
    ///
    /// Uses SecureStream to handle:
    /// - Challenge-response authentication
    /// - Bidirectional sender_key exchange
    /// - Deadlock prevention (deterministic role assignment)
    ///
    /// Old implementation: ~300 lines of manual auth protocol.
    /// New implementation: ~10 lines calling SecureStream.
    pub(super) async fn initiate_key_share_process(
        &self,
        context: &mut Context,
        our_identity: PublicKey,
        stream: &mut Stream,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            %our_identity,
            "Initiating key share with SecureStream authentication",
        );

        // SecureStream handles the entire authentication + key exchange:
        // - Identity exchange
        // - Challenge-response authentication (bidirectional)
        // - sender_key exchange
        // - Deadlock prevention via deterministic role assignment
        SecureStream::authenticate_p2p(
            stream,
            context,
            our_identity,
            &self.context_client,
            self.sync_config.timeout,
        )
        .await
        // Authentication complete! sender_keys exchanged and identities updated.
    }

    /// Handle incoming key share request from a peer.
    ///
    /// Responds to the peer's key share request and performs mutual authentication.
    ///
    /// **CRITICAL**: This is called AFTER the manager consumed the Init message.
    /// We must NOT send Init again - just acknowledge and proceed with auth.
    pub(super) async fn handle_key_share_request(
        &self,
        context: &Context,
        our_identity: PublicKey,
        their_identity: PublicKey,
        stream: &mut Stream,
        their_nonce: calimero_crypto::Nonce,
    ) -> eyre::Result<()> {
        info!(
            context_id=%context.id,
            %our_identity,
            %their_identity,
            "Handling key share request (responding to peer's init)",
        );
        
        // Send acknowledgment (we received their Init, send ours back)
        let our_nonce = thread_rng().gen::<calimero_crypto::Nonce>();
        
        super::stream::send(
            stream,
            &calimero_node_primitives::sync::StreamMessage::Init {
                context_id: context.id,
                party_id: our_identity,
                payload: calimero_node_primitives::sync::InitPayload::KeyShare,
                next_nonce: our_nonce,
            },
            None,
        )
        .await?;
        
        // Now proceed with authentication (both sides past Init phase)
        SecureStream::authenticate_p2p_after_init(
            stream,
            context,
            our_identity,
            their_identity,
            our_nonce,
            their_nonce,
            &self.context_client,
            self.sync_config.timeout,
        )
        .await
        // Authentication complete! sender_keys exchanged and identities updated.
    }
}
