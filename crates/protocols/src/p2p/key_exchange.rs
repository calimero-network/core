//! Key Exchange Protocol - Exchange encryption keys between peers
//!
//! **Purpose**: Allow peers to exchange sender_keys for delta encryption.
//!
//! **Protocol**: Uses SecureStream for mutual authentication, then exchanges keys.
//!
//! **Stateless Design**: All dependencies injected as parameters (NO SyncManager!)

use calimero_context_primitives::client::ContextClient;
use calimero_network_primitives::client::NetworkClient;
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::sync::{InitPayload, StreamMessage};
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::Result;
use rand::{thread_rng, Rng};
use tokio::time::Duration;
use tracing::{debug, info, warn};

use crate::stream::SecureStream;

// ═══════════════════════════════════════════════════════════════════════════
// Client Side: Request Key Exchange
// ═══════════════════════════════════════════════════════════════════════════

/// Request key exchange with a peer (client side).
///
/// Opens a stream, sends Init with KeyShare payload, and performs mutual authentication.
/// SecureStream automatically exchanges sender_keys during authentication.
///
/// # Arguments
/// * `network_client` - Network client to open streams
/// * `context` - Context for the key exchange
/// * `our_identity` - Our identity in this context
/// * `peer_id` - Peer to exchange keys with
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for the exchange
///
/// # Example
/// ```rust,ignore
/// request_key_exchange(
///     &network_client,
///     &context,
///     our_identity,
///     peer_id,
///     &context_client,
///     Duration::from_secs(10),
/// ).await?;
/// // sender_keys are now exchanged!
/// ```
pub async fn request_key_exchange(
    network_client: &NetworkClient,
    context: &Context,
    our_identity: PublicKey,
    peer_id: libp2p::PeerId,
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<()> {
    info!(
        context_id=%context.id,
        %our_identity,
        peer_id=%peer_id,
        "Initiating key exchange with peer"
    );

    let mut stream = network_client.open_stream(peer_id).await?;

    // SecureStream handles Init exchange + authentication + key exchange
    // (It will send the Init message internally)
    SecureStream::authenticate_p2p(&mut stream, context, our_identity, context_client, timeout)
        .await?;

    info!(
        context_id=%context.id,
        %our_identity,
        peer_id=%peer_id,
        "Key exchange completed successfully"
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Server Side: Handle Key Exchange Request
// ═══════════════════════════════════════════════════════════════════════════

/// Handle incoming key exchange request (server side).
///
/// Called when a peer sends Init with KeyShare payload.
/// Responds with acknowledgment and performs mutual authentication.
///
/// **NOTE**: This is called AFTER the Init message has been consumed.
/// We send our own Init as acknowledgment, then proceed with auth.
///
/// # Arguments
/// * `stream` - Stream (Init message already consumed)
/// * `context` - Context for the key exchange
/// * `our_identity` - Our identity in this context
/// * `their_identity` - Their identity (from their Init message)
/// * `their_nonce` - Their nonce (from their Init message)
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for the exchange
pub async fn handle_key_exchange(
    stream: &mut Stream,
    context: &Context,
    our_identity: PublicKey,
    their_identity: PublicKey,
    their_nonce: calimero_crypto::Nonce,
    context_client: &ContextClient,
    timeout: Duration,
) -> Result<()> {
    info!(
        context_id=%context.id,
        %our_identity,
        %their_identity,
        "Handling key exchange request (responding to peer's Init)",
    );

    // Note: sync_context_config is now called in the Subscribed event handler
    // We assume the identity already exists here. If not, the error will be caught below.

    info!(
        context_id=%context.id,
        %our_identity,
        %their_identity,
        "SERVER: Sending Init ack"
    );

    // Send acknowledgment (we received their Init, send ours back)
    let our_nonce = thread_rng().gen::<calimero_crypto::Nonce>();

    crate::stream::send(
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

    info!(
        context_id=%context.id,
        %our_identity,
        %their_identity,
        "SERVER: Init ack sent, calling authenticate_p2p_after_init"
    );

    // Perform authentication (both sides past Init phase)
    SecureStream::authenticate_p2p_after_init(
        stream,
        context,
        our_identity,
        their_identity,
        our_nonce,
        their_nonce,
        context_client,
        timeout,
    )
    .await?;

    info!(
        context_id=%context.id,
        %our_identity,
        %their_identity,
        "Key exchange completed - sender_keys exchanged"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_key_exchange_protocol() {
        // TODO: Test bidirectional key exchange
        // - Create two connected streams
        // - Run request_key_exchange on one side
        // - Run handle_key_exchange on other side
        // - Verify sender_keys are exchanged and stored
    }
}
