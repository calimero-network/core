//! Blob Request Protocol - Stateless P2P blob sharing handlers
//!
//! **Purpose**: Share blobs (large binary data) between context members.
//!
//! **Protocol**:
//! 1. Client sends Init with BlobShare payload
//! 2. Client proves identity (prevents unauthorized access)
//! 3. Server verifies identity, streams blob chunks
//! 4. Client receives chunks and stores blob
//!
//! **Stateless Design**: All dependencies injected as parameters (NO SyncManager!)

use calimero_context_primitives::client::ContextClient;
use calimero_crypto::{Nonce, SharedKey, NONCE_LEN};
use calimero_network_primitives::stream::Stream;
use calimero_node_primitives::client::NodeClient;
use calimero_node_primitives::sync::{InitPayload, MessagePayload, StreamMessage};
use calimero_primitives::blobs::BlobId;
use calimero_primitives::context::Context;
use calimero_primitives::identity::PublicKey;
use eyre::{bail, OptionExt};
use futures_util::stream::poll_fn;
use futures_util::TryStreamExt;
use rand::{thread_rng, Rng};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{info, warn};

use crate::stream::{SecureStream, Sequencer};

// ═══════════════════════════════════════════════════════════════════════════
// Client Side: Request Blob
// ═══════════════════════════════════════════════════════════════════════════

/// Request a blob from a peer and add it to local storage (client side).
///
/// Opens encrypted channel, streams chunks, verifies blob ID matches.
///
/// # Arguments
/// * `stream` - Open stream to the peer
/// * `context` - Context for the blob
/// * `our_identity` - Our identity in this context
/// * `blob_id` - ID of the blob to request
/// * `size` - Expected size of the blob
/// * `node_client` - Client for storing blob
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for the request
pub async fn request_blob(
    stream: &mut Stream,
    context: &Context,
    our_identity: PublicKey,
    blob_id: BlobId,
    size: u64,
    node_client: &NodeClient,
    context_client: &ContextClient,
    timeout: Duration,
) -> eyre::Result<()> {
    info!(
        context_id=%context.id,
        our_identity=%our_identity,
        blob_id=%blob_id,
        "Initiating blob share",
    );

    let our_nonce = thread_rng().gen::<Nonce>();

    crate::stream::send(
        stream,
        &StreamMessage::Init {
            context_id: context.id,
            party_id: our_identity,
            payload: InitPayload::BlobShare { blob_id },
            next_nonce: our_nonce,
        },
        None,
    )
    .await?;

    // SECURITY: Prove our identity ownership before peer serves blob
    SecureStream::prove_identity(stream, &context.id, &our_identity, context_client, timeout)
        .await
        .map_err(|e| eyre::eyre!("Failed to prove identity for blob request: {}", e))?;

    let Some(ack) = crate::stream::recv(stream, None, timeout).await? else {
        bail!("connection closed while awaiting blob share handshake");
    };

    let (their_identity, mut their_nonce) = match ack {
        StreamMessage::Init {
            party_id,
            payload: InitPayload::BlobShare {
                blob_id: ack_blob_id,
            },
            next_nonce,
            ..
        } => {
            if ack_blob_id != blob_id {
                bail!(
                    "unexpected ack blob id: expected {}, got {}",
                    blob_id,
                    ack_blob_id
                );
            }

            (party_id, next_nonce)
        }
        unexpected @ (StreamMessage::Init { .. }
        | StreamMessage::Message { .. }
        | StreamMessage::OpaqueError) => {
            bail!("unexpected message: {:?}", unexpected)
        }
    };

    let private_key = context_client
        .get_identity(&context.id, &our_identity)?
        .and_then(|i| i.private_key)
        .ok_or_eyre("expected own identity to have private key")?;

    let shared_key = SharedKey::new(&private_key, &their_identity);

    let (tx, mut rx) = mpsc::channel(1);

    let add_task = node_client.add_blob(
        poll_fn(|cx| rx.poll_recv(cx)).into_async_read(),
        Some(size),
        None,
    );

    let read_task = async {
        let mut sequencer = Sequencer::default();

        while let Some(msg) =
            crate::stream::recv(stream, Some((shared_key, their_nonce)), timeout).await?
        {
            let (sequence_id, chunk, their_new_nonce) = match msg {
                StreamMessage::OpaqueError => bail!("other peer ran into an error"),
                StreamMessage::Message {
                    sequence_id,
                    payload: MessagePayload::BlobShare { chunk },
                    next_nonce,
                } => (sequence_id, chunk, next_nonce),
                unexpected @ (StreamMessage::Init { .. } | StreamMessage::Message { .. }) => {
                    bail!("unexpected message: {:?}", unexpected)
                }
            };

            sequencer.expect(sequence_id)?;

            if chunk.is_empty() {
                break;
            }

            tx.send(Ok(chunk)).await?;

            their_nonce = their_new_nonce;
        }

        drop(tx);

        Ok(())
    };

    let ((received_blob_id, _), _) = tokio::try_join!(add_task, read_task)?;

    if received_blob_id != blob_id {
        bail!(
            "unexpected blob id: expected {}, got {}",
            blob_id,
            received_blob_id
        );
    }

    info!(
        context_id=%context.id,
        our_identity=%our_identity,
        their_identity=%their_identity,
        blob_id=%blob_id,
        "Blob share completed",
    );

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Server Side: Handle Blob Request
// ═══════════════════════════════════════════════════════════════════════════

/// Handle incoming blob request from a peer (server side).
///
/// Verifies identity, fetches blob, streams chunks over encrypted channel.
///
/// **NOTE**: This is called AFTER the Init message has been consumed.
///
/// # Arguments
/// * `stream` - Stream (Init message already consumed)
/// * `context` - Context for the blob
/// * `our_identity` - Our identity in this context
/// * `their_identity` - Their identity (from their Init message)
/// * `blob_id` - ID of the blob being requested
/// * `node_client` - Client for fetching blob
/// * `context_client` - Client for identity operations
/// * `timeout` - Timeout for verification
pub async fn handle_blob_request(
    stream: &mut Stream,
    context: &Context,
    our_identity: PublicKey,
    their_identity: PublicKey,
    blob_id: BlobId,
    node_client: &NodeClient,
    context_client: &ContextClient,
    timeout: Duration,
) -> eyre::Result<()> {
    info!(
        context_id=%context.id,
        our_identity=%our_identity,
        their_identity=%their_identity,
        blob_id=%blob_id,
        "Received blob share request - verifying requester identity",
    );

    // SECURITY: Verify requester actually owns the identity they claimed
    // This prevents non-members from requesting blobs (metadata leak prevention)
    SecureStream::verify_identity(
        stream,
        &context.id,
        &their_identity,
        &our_identity,
        context_client,
        timeout,
    )
    .await
    .map_err(|e| eyre::eyre!("Blob request denied - identity verification failed: {}", e))?;

    info!(
        context_id=%context.id,
        their_identity=%their_identity,
        blob_id=%blob_id,
        "Identity verified - serving blob"
    );

    let Some(mut blob) = node_client.get_blob(&blob_id, None).await? else {
        warn!(%blob_id, "blob not found");

        return Ok(());
    };

    let private_key = context_client
        .get_identity(&context.id, &our_identity)?
        .and_then(|i| i.private_key)
        .ok_or_eyre("expected own identity to have private key")?;

    let shared_key = SharedKey::new(&private_key, &their_identity);
    let mut our_nonce = thread_rng().gen::<Nonce>();

    crate::stream::send(
        stream,
        &StreamMessage::Init {
            context_id: context.id,
            party_id: our_identity,
            payload: InitPayload::BlobShare { blob_id },
            next_nonce: our_nonce,
        },
        None,
    )
    .await?;

    let mut sequencer = Sequencer::default();

    while let Some(chunk) = blob.try_next().await? {
        let our_new_nonce = thread_rng().gen::<Nonce>();
        crate::stream::send(
            stream,
            &StreamMessage::Message {
                sequence_id: sequencer.next(),
                payload: MessagePayload::BlobShare {
                    chunk: chunk.into_vec().into(),
                },
                next_nonce: our_new_nonce,
            },
            Some((shared_key, our_nonce)),
        )
        .await?;

        our_nonce = our_new_nonce;
    }

    crate::stream::send(
        stream,
        &StreamMessage::Message {
            sequence_id: sequencer.next(),
            payload: MessagePayload::BlobShare { chunk: b"".into() },
            next_nonce: [0; NONCE_LEN],
        },
        Some((shared_key, our_nonce)),
    )
    .await?;

    info!(
        context_id=%context.id,
        our_identity=%our_identity,
        their_identity=%their_identity,
        blob_id=%blob_id,
        "Blob share completed",
    );

    Ok(())
}
