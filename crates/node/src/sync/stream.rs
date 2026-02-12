//! Stream communication utilities.
//!
//! **Single Responsibility**: Handle stream send/recv with encryption/decryption.
//!
//! ## DRY Principle
//!
//! This module extracts the repeated send/recv logic that was duplicated across
//! all protocol files. Every protocol needs to send/recv encrypted messages.
//!
//! ## Transport Abstraction
//!
//! [`StreamTransport`] implements [`SyncTransport`] for production libp2p streams,
//! enabling the same protocol code to work with both production and simulation.

use std::time::Duration;

use async_trait::async_trait;
use calimero_crypto::{Nonce, SharedKey};
use calimero_network_primitives::stream::{Message, Stream};
use calimero_node_primitives::sync::{EncryptionState, StreamMessage, SyncTransport};
use eyre::{OptionExt, WrapErr};
use futures_util::{SinkExt, TryStreamExt};
use tokio::time::timeout;

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

// =============================================================================
// StreamTransport - SyncTransport implementation for libp2p Stream
// =============================================================================

/// Default timeout for receive operations.
const DEFAULT_RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// Transport wrapper for libp2p [`Stream`] implementing [`SyncTransport`].
///
/// This enables production sync protocols to be generic over transport,
/// allowing the same code to work with both libp2p streams and simulation.
///
/// # Example
///
/// ```ignore
/// let stream = open_stream(peer_id).await?;
/// let mut transport = StreamTransport::new(stream);
///
/// // Set encryption after key exchange
/// transport.set_encryption(Some((shared_key, nonce)));
///
/// // Use with protocol
/// hash_comparison_sync(&mut transport, context_id, ...).await?;
/// ```
pub struct StreamTransport<'a> {
    /// The underlying libp2p stream (mutable reference).
    stream: &'a mut Stream,
    /// Encryption state.
    encryption: EncryptionState,
    /// Default timeout for recv operations.
    recv_timeout: Duration,
}

impl<'a> StreamTransport<'a> {
    /// Create a new transport wrapper around a libp2p stream.
    #[must_use]
    pub fn new(stream: &'a mut Stream) -> Self {
        Self {
            stream,
            encryption: EncryptionState::new(),
            recv_timeout: DEFAULT_RECV_TIMEOUT,
        }
    }

    /// Create with a custom default receive timeout.
    #[must_use]
    #[expect(dead_code, reason = "Future API for custom timeouts")]
    pub fn with_timeout(stream: &'a mut Stream, timeout: Duration) -> Self {
        Self {
            stream,
            encryption: EncryptionState::new(),
            recv_timeout: timeout,
        }
    }
}

#[async_trait]
impl SyncTransport for StreamTransport<'_> {
    async fn send(&mut self, message: &StreamMessage<'_>) -> eyre::Result<()> {
        let encoded = borsh::to_vec(message)?;
        let encrypted = self.encryption.encrypt(encoded)?;
        self.stream.send(Message::new(encrypted)).await?;
        Ok(())
    }

    async fn recv(&mut self) -> eyre::Result<Option<StreamMessage<'static>>> {
        self.recv_timeout(self.recv_timeout).await
    }

    async fn recv_timeout(
        &mut self,
        budget: Duration,
    ) -> eyre::Result<Option<StreamMessage<'static>>> {
        let message = timeout(budget, self.stream.try_next())
            .await
            .wrap_err("timeout receiving message from peer")?
            .wrap_err("error receiving message from peer")?;

        let Some(message) = message else {
            return Ok(None);
        };

        let message = message.data.into_owned();
        let decrypted = self.encryption.decrypt(message)?;
        let decoded = borsh::from_slice::<StreamMessage<'static>>(&decrypted)?;

        Ok(Some(decoded))
    }

    fn set_encryption(&mut self, encryption: Option<(SharedKey, Nonce)>) {
        self.encryption.set(encryption);
    }

    fn encryption(&self) -> Option<(SharedKey, Nonce)> {
        self.encryption.get()
    }

    async fn close(&mut self) -> eyre::Result<()> {
        self.stream.close().await?;
        Ok(())
    }
}
