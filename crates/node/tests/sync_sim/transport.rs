//! Simulated transport for testing sync protocols.
//!
//! Provides [`SimStream`], an in-memory implementation of [`SyncTransport`]
//! that enables running the production sync protocol code in simulation.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────┐                    ┌─────────────────┐
//! │    SimNode A    │                    │    SimNode B    │
//! │                 │                    │                 │
//! │  SimStream      │                    │  SimStream      │
//! │  ┌───────────┐  │                    │  ┌───────────┐  │
//! │  │  tx_a ────┼──┼────────────────────┼──┼─► rx_b    │  │
//! │  │  rx_a ◄───┼──┼────────────────────┼──┼── tx_b    │  │
//! │  └───────────┘  │                    │  └───────────┘  │
//! └─────────────────┘                    └─────────────────┘
//! ```
//!
//! # Usage
//!
//! ```ignore
//! // Create bidirectional channel pair
//! let (stream_a, stream_b) = SimStream::pair();
//!
//! // Run protocol on both ends
//! let initiator = async { hash_comparison_sync(&mut stream_a, ...).await };
//! let responder = async { handle_tree_node_request(&mut stream_b, ...).await };
//!
//! // Execute concurrently
//! tokio::join!(initiator, responder);
//! ```

use std::collections::VecDeque;
use std::time::Duration;

use async_trait::async_trait;
use calimero_crypto::{Nonce, SharedKey};
use calimero_node_primitives::sync::{EncryptionState, StreamMessage, SyncTransport};
use eyre::{bail, Result};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Default channel buffer size.
const DEFAULT_BUFFER_SIZE: usize = 64;

/// Default timeout for receive operations in simulation.
const DEFAULT_SIM_TIMEOUT: Duration = Duration::from_secs(5);

/// In-memory transport for simulation testing.
///
/// Implements [`SyncTransport`] using tokio mpsc channels, enabling
/// the production sync protocol code to run in simulation.
///
/// # Features
///
/// - Bidirectional communication via channel pairs
/// - Optional message buffering/queueing for testing
/// - Configurable timeouts
/// - Optional encryption (for testing encrypted flows)
pub struct SimStream {
    /// Sender channel (outgoing messages).
    /// Option so we can drop it to signal closure.
    tx: Option<mpsc::Sender<Vec<u8>>>,
    /// Receiver channel (incoming messages).
    rx: mpsc::Receiver<Vec<u8>>,
    /// Buffer for received messages (allows peek/reorder testing).
    buffer: VecDeque<Vec<u8>>,
    /// Encryption state.
    encryption: EncryptionState,
    /// Default timeout for receive operations.
    recv_timeout: Duration,
    /// Whether the stream is closed.
    closed: bool,
}

impl SimStream {
    /// Create a bidirectional stream pair for two-party communication.
    ///
    /// Returns `(stream_a, stream_b)` where:
    /// - Messages sent on `stream_a` are received on `stream_b`
    /// - Messages sent on `stream_b` are received on `stream_a`
    ///
    /// # Example
    ///
    /// ```ignore
    /// let (mut alice_stream, mut bob_stream) = SimStream::pair();
    ///
    /// // Alice sends, Bob receives
    /// alice_stream.send(&msg).await?;
    /// let received = bob_stream.recv().await?;
    /// ```
    #[must_use]
    pub fn pair() -> (Self, Self) {
        Self::pair_with_buffer(DEFAULT_BUFFER_SIZE)
    }

    /// Create a stream pair with custom buffer size.
    ///
    /// Larger buffers allow more in-flight messages before backpressure.
    #[must_use]
    pub fn pair_with_buffer(buffer_size: usize) -> (Self, Self) {
        let (tx_a, rx_b) = mpsc::channel(buffer_size);
        let (tx_b, rx_a) = mpsc::channel(buffer_size);

        let stream_a = Self {
            tx: Some(tx_a),
            rx: rx_a,
            buffer: VecDeque::new(),
            encryption: EncryptionState::new(),
            recv_timeout: DEFAULT_SIM_TIMEOUT,
            closed: false,
        };

        let stream_b = Self {
            tx: Some(tx_b),
            rx: rx_b,
            buffer: VecDeque::new(),
            encryption: EncryptionState::new(),
            recv_timeout: DEFAULT_SIM_TIMEOUT,
            closed: false,
        };

        (stream_a, stream_b)
    }

    /// Create a one-way stream (for testing responder-only scenarios).
    ///
    /// Returns `(sender, receiver)` where sender can only send and receiver can only receive.
    #[must_use]
    pub fn one_way() -> (SimStreamSender, Self) {
        Self::one_way_with_buffer(DEFAULT_BUFFER_SIZE)
    }

    /// Create a one-way stream with custom buffer size.
    #[must_use]
    pub fn one_way_with_buffer(buffer_size: usize) -> (SimStreamSender, Self) {
        let (tx, rx) = mpsc::channel(buffer_size);

        let sender = SimStreamSender {
            tx,
            encryption: EncryptionState::new(),
        };

        let receiver = Self {
            tx: None, // No sender for one-way receiver
            rx,
            buffer: VecDeque::new(),
            encryption: EncryptionState::new(),
            recv_timeout: DEFAULT_SIM_TIMEOUT,
            closed: false,
        };

        (sender, receiver)
    }

    /// Set the default receive timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.recv_timeout = timeout;
    }

    /// Check if there are buffered messages.
    #[must_use]
    pub fn has_buffered(&self) -> bool {
        !self.buffer.is_empty()
    }

    /// Get count of buffered messages.
    #[must_use]
    pub fn buffered_count(&self) -> usize {
        self.buffer.len()
    }

    /// Internal: receive raw bytes with timeout.
    async fn recv_raw_timeout(&mut self, budget: Duration) -> Result<Option<Vec<u8>>> {
        // First check buffer
        if let Some(data) = self.buffer.pop_front() {
            return Ok(Some(data));
        }

        if self.closed {
            return Ok(None);
        }

        // Then try channel with timeout
        match timeout(budget, self.rx.recv()).await {
            Ok(Some(data)) => Ok(Some(data)),
            Ok(None) => {
                self.closed = true;
                Ok(None)
            }
            Err(_) => bail!("timeout receiving message"),
        }
    }
}

#[async_trait]
impl SyncTransport for SimStream {
    async fn send(&mut self, message: &StreamMessage<'_>) -> Result<()> {
        if self.closed {
            bail!("stream is closed");
        }

        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| eyre::eyre!("no sender available"))?;

        let encoded = borsh::to_vec(message)?;
        let encrypted = self.encryption.encrypt(encoded)?;

        tx.send(encrypted)
            .await
            .map_err(|_| eyre::eyre!("channel closed"))?;

        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<StreamMessage<'static>>> {
        self.recv_timeout(self.recv_timeout).await
    }

    async fn recv_timeout(&mut self, budget: Duration) -> Result<Option<StreamMessage<'static>>> {
        let Some(data) = self.recv_raw_timeout(budget).await? else {
            return Ok(None);
        };

        let decrypted = self.encryption.decrypt(data)?;
        let decoded = borsh::from_slice::<StreamMessage<'static>>(&decrypted)?;

        Ok(Some(decoded))
    }

    fn set_encryption(&mut self, encryption: Option<(SharedKey, Nonce)>) {
        self.encryption.set(encryption);
    }

    fn encryption(&self) -> Option<(SharedKey, Nonce)> {
        self.encryption.get()
    }

    async fn close(&mut self) -> Result<()> {
        self.closed = true;
        // Drop the sender to signal closure to the other end
        // When all senders are dropped, the receiver will see None
        self.tx = None;
        Ok(())
    }
}

impl std::fmt::Debug for SimStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SimStream")
            .field("buffered", &self.buffer.len())
            .field("closed", &self.closed)
            .field("has_encryption", &self.encryption.get().is_some())
            .finish()
    }
}

// =============================================================================
// One-way sender (for testing)
// =============================================================================

/// One-way sender for testing scenarios.
///
/// Can only send messages, not receive.
pub struct SimStreamSender {
    tx: mpsc::Sender<Vec<u8>>,
    encryption: EncryptionState,
}

impl SimStreamSender {
    /// Send a message.
    ///
    /// # Errors
    ///
    /// Returns error if serialization, encryption, or send fails.
    pub async fn send(&self, message: &StreamMessage<'_>) -> Result<()> {
        let encoded = borsh::to_vec(message)?;
        let encrypted = self.encryption.encrypt(encoded)?;

        self.tx
            .send(encrypted)
            .await
            .map_err(|_| eyre::eyre!("channel closed"))?;

        Ok(())
    }

    /// Set encryption parameters.
    pub fn set_encryption(&mut self, encryption: Option<(SharedKey, Nonce)>) {
        self.encryption.set(encryption);
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use calimero_crypto::NONCE_LEN;
    use calimero_node_primitives::sync::wire::{InitPayload, MessagePayload};
    use calimero_primitives::context::ContextId;
    use calimero_primitives::identity::PublicKey;

    fn test_context_id() -> ContextId {
        ContextId::from([1u8; 32])
    }

    fn test_public_key() -> PublicKey {
        PublicKey::from([2u8; 32])
    }

    #[tokio::test]
    async fn test_pair_send_recv() {
        let (mut alice, mut bob) = SimStream::pair();

        let msg = StreamMessage::Init {
            context_id: test_context_id(),
            party_id: test_public_key(),
            payload: InitPayload::DagHeadsRequest {
                context_id: test_context_id(),
            },
            next_nonce: [0; NONCE_LEN],
        };

        // Alice sends
        alice.send(&msg).await.expect("send should succeed");

        // Bob receives
        let received = bob.recv().await.expect("recv should succeed");
        assert!(received.is_some());

        // Verify message matches (compare serialized form since StreamMessage doesn't impl Eq)
        let original_bytes = borsh::to_vec(&msg).unwrap();
        let received_bytes = borsh::to_vec(&received.unwrap()).unwrap();
        assert_eq!(original_bytes, received_bytes);
    }

    #[tokio::test]
    async fn test_bidirectional() {
        let (mut alice, mut bob) = SimStream::pair();

        let msg_from_alice = StreamMessage::Init {
            context_id: test_context_id(),
            party_id: test_public_key(),
            payload: InitPayload::DagHeadsRequest {
                context_id: test_context_id(),
            },
            next_nonce: [1; NONCE_LEN],
        };

        let msg_from_bob = StreamMessage::Message {
            sequence_id: 1,
            payload: MessagePayload::DagHeadsResponse {
                dag_heads: vec![],
                root_hash: [0u8; 32].into(),
            },
            next_nonce: [2; NONCE_LEN],
        };

        // Send both directions
        alice.send(&msg_from_alice).await.unwrap();
        bob.send(&msg_from_bob).await.unwrap();

        // Receive both directions
        let from_alice = bob.recv().await.unwrap().unwrap();
        let from_bob = alice.recv().await.unwrap().unwrap();

        // Verify
        assert_eq!(
            borsh::to_vec(&msg_from_alice).unwrap(),
            borsh::to_vec(&from_alice).unwrap()
        );
        assert_eq!(
            borsh::to_vec(&msg_from_bob).unwrap(),
            borsh::to_vec(&from_bob).unwrap()
        );
    }

    #[tokio::test]
    async fn test_timeout() {
        let (mut _alice, mut bob) = SimStream::pair();

        // Bob tries to receive with short timeout, should fail
        let result = bob.recv_timeout(Duration::from_millis(10)).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timeout"));
    }

    #[tokio::test]
    async fn test_close() {
        let (mut alice, mut bob) = SimStream::pair();

        // Close Alice's stream
        alice.close().await.unwrap();

        // Sending should fail
        let msg = StreamMessage::Init {
            context_id: test_context_id(),
            party_id: test_public_key(),
            payload: InitPayload::DagHeadsRequest {
                context_id: test_context_id(),
            },
            next_nonce: [0; NONCE_LEN],
        };
        let result = alice.send(&msg).await;
        assert!(result.is_err());

        // Bob should eventually see closed channel (after timeout)
        bob.set_timeout(Duration::from_millis(50));
        // Note: recv returns None when channel is closed after draining
    }
}
