//! Stream communication utilities.
//!
//! **Single Responsibility**: Handle stream send/recv with encryption/decryption.
//!
//! ## DRY Principle
//!
//! This module extracts the repeated send/recv logic that was duplicated across
//! all protocol files. Every protocol needs to send/recv encrypted messages.

use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::stream::{Message, Stream};
use calimero_node_primitives::sync::StreamMessage;
use eyre::{OptionExt, WrapErr};
use futures_util::{SinkExt, TryStreamExt};
use tokio::time::{timeout, Duration};

/// Sends an encrypted message over a stream.
///
/// # Arguments
///
/// * `stream` - The network stream
/// * `message` - The message to send
/// * `shared_key` - Optional encryption (key, nonce)
///
/// # Errors
///
/// Returns error if serialization, encryption, or network send fails.
pub async fn send(
    stream: &mut Stream,
    message: &StreamMessage<'_>,
    shared_key: Option<(SharedKey, Nonce)>,
) -> eyre::Result<()> {
    let encoded = borsh::to_vec(message)?;

    let message = match shared_key {
        Some((key, nonce)) => key
            .encrypt(encoded, nonce)
            .ok_or_eyre("encryption failed")?,
        None => encoded,
    };

    stream.send(Message::new(message)).await?;

    Ok(())
}

/// Receives and decrypts a message from a stream.
///
/// # Arguments
///
/// * `stream` - The network stream
/// * `shared_key` - Optional decryption (key, nonce)
/// * `budget` - Timeout duration
///
/// # Errors
///
/// Returns error if network receive, decryption, or deserialization fails.
pub async fn recv(
    stream: &mut Stream,
    shared_key: Option<(SharedKey, Nonce)>,
    budget: Duration,
) -> eyre::Result<Option<StreamMessage<'static>>> {
    let message = timeout(budget, stream.try_next())
        .await
        .wrap_err("timeout receiving message from peer")?
        .wrap_err("error receiving message from peer")?;

    let Some(message) = message else {
        return Ok(None);
    };

    let message = message.data.into_owned();

    let decrypted = match shared_key {
        Some((key, nonce)) => key
            .decrypt(message, nonce)
            .ok_or_eyre("decryption failed")?,
        None => message,
    };

    let decoded = borsh::from_slice::<StreamMessage<'static>>(&decrypted)?;

    Ok(Some(decoded))
}
