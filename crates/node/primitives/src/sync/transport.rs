//! Transport abstraction for sync protocols.
//!
//! This module provides the [`SyncTransport`] trait that abstracts the underlying
//! network transport, enabling:
//!
//! - Production code to use libp2p streams
//! - Simulation tests to use in-memory channels
//! - Same protocol code to run in both environments
//!
//! # Design Rationale
//!
//! The sync protocol code needs to send and receive [`StreamMessage`] payloads.
//! By abstracting this behind a trait, we can:
//!
//! 1. Test the actual production protocol logic in simulation
//! 2. Verify invariants (I4, I5, I6) with real message flow
//! 3. Inject network faults (latency, loss, reorder) in tests
//!
//! # Example
//!
//! ```ignore
//! async fn hash_comparison_sync<T: SyncTransport>(
//!     transport: &mut T,
//!     context_id: ContextId,
//!     // ...
//! ) -> Result<Stats> {
//!     transport.send(&request_msg).await?;
//!     let response = transport.recv().await?;
//!     // ...
//! }
//! ```

use std::time::Duration;

use async_trait::async_trait;
use calimero_crypto::{Nonce, SharedKey};
use eyre::Result;

use super::wire::StreamMessage;

/// Transport abstraction for sync protocol message exchange.
///
/// Implementations handle serialization, optional encryption, and the
/// underlying transport mechanism (network streams or in-memory channels).
///
/// # Encryption
///
/// Transport implementations may support optional encryption. Use
/// [`set_encryption`](SyncTransport::set_encryption) to configure the
/// shared key and nonce for encrypted communication.
#[async_trait]
pub trait SyncTransport: Send {
    /// Send a message to the peer.
    ///
    /// The implementation handles serialization and optional encryption.
    ///
    /// # Errors
    ///
    /// Returns error if serialization, encryption, or send fails.
    async fn send(&mut self, message: &StreamMessage<'_>) -> Result<()>;

    /// Receive a message from the peer.
    ///
    /// The implementation handles deserialization and optional decryption.
    ///
    /// # Errors
    ///
    /// Returns error if receive, decryption, or deserialization fails.
    /// Returns `Ok(None)` if the stream is closed.
    async fn recv(&mut self) -> Result<Option<StreamMessage<'static>>>;

    /// Receive a message with a timeout.
    ///
    /// # Errors
    ///
    /// Returns error if timeout expires or receive fails.
    async fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<StreamMessage<'static>>>;

    /// Set encryption parameters for subsequent send/recv operations.
    ///
    /// Pass `None` to disable encryption.
    fn set_encryption(&mut self, encryption: Option<(SharedKey, Nonce)>);

    /// Get the current encryption parameters.
    fn encryption(&self) -> Option<(SharedKey, Nonce)>;

    /// Close the transport.
    ///
    /// After closing, further send/recv calls will fail.
    async fn close(&mut self) -> Result<()>;
}

// =============================================================================
// Encryption Helper
// =============================================================================

/// Common encryption state that implementations can embed.
#[derive(Debug, Clone, Default)]
pub struct EncryptionState {
    /// Current encryption key and nonce.
    pub key_nonce: Option<(SharedKey, Nonce)>,
}

impl EncryptionState {
    /// Create new encryption state (no encryption).
    #[must_use]
    pub fn new() -> Self {
        Self { key_nonce: None }
    }

    /// Set encryption parameters.
    pub fn set(&mut self, encryption: Option<(SharedKey, Nonce)>) {
        self.key_nonce = encryption;
    }

    /// Get current encryption parameters.
    #[must_use]
    pub fn get(&self) -> Option<(SharedKey, Nonce)> {
        self.key_nonce.clone()
    }

    /// Encrypt data if encryption is configured.
    ///
    /// # Errors
    ///
    /// Returns error if encryption fails.
    pub fn encrypt(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        match &self.key_nonce {
            Some((key, nonce)) => key
                .encrypt(data, *nonce)
                .ok_or_else(|| eyre::eyre!("encryption failed")),
            None => Ok(data),
        }
    }

    /// Decrypt data if encryption is configured.
    ///
    /// # Errors
    ///
    /// Returns error if decryption fails.
    pub fn decrypt(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        match &self.key_nonce {
            Some((key, nonce)) => key
                .decrypt(data, *nonce)
                .ok_or_else(|| eyre::eyre!("decryption failed")),
            None => Ok(data),
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encryption_state_default() {
        let state = EncryptionState::new();
        assert!(state.get().is_none());
    }

    #[test]
    fn test_encryption_state_passthrough() {
        let state = EncryptionState::new();
        let data = b"hello world".to_vec();
        let encrypted = state.encrypt(data.clone()).unwrap();
        assert_eq!(encrypted, data); // No encryption = passthrough
        let decrypted = state.decrypt(encrypted).unwrap();
        assert_eq!(decrypted, data);
    }
}
