//! Calimero Network Protocols
//!
//! Stateless protocol handlers for Calimero node communication.
//!
//! This crate provides pure, stateless protocol handlers that can be tested in isolation.
//! All state is injected as parameters - no hidden dependencies, no actors, no framework magic.
//!
//! # Architecture
//!
//! ## Modules
//!
//! - [`gossipsub`]: Broadcast protocols (one-to-many, encrypted)
//!   - [`gossipsub::state_delta`]: State delta broadcast handler with DAG cascade logic
//!
//! - [`p2p`]: Request/response protocols (one-to-one, authenticated)
//!   - [`p2p::key_exchange`]: Bidirectional key exchange with SecureStream
//!   - [`p2p::delta_request`]: DAG gap filling (request missing deltas)
//!   - [`p2p::blob_request`]: Context-authenticated blob sharing
//!   - [`p2p::blob_protocol`]: Public blob download (CALIMERO_BLOB_PROTOCOL)
//!
//! - [`stream`]: Authenticated stream utilities (always secure)
//!   - [`stream::SecureStream`]: Challenge-response P2P authentication
//!   - [`stream::Sequencer`]: Message sequence tracking
//!
//! # Design Principles
//!
//! 1. **Stateless**: All handlers are pure functions (state injected as params)
//! 2. **Testable**: No dependencies on full node infrastructure
//! 3. **Reusable**: Can be used in different contexts (node, tests, tools)
//! 4. **Secure by default**: All P2P uses authenticated streams
//! 5. **No actors**: Plain async Rust (no framework coupling)
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use calimero_protocols::p2p::key_exchange::request_key_exchange;
//!
//! // Request key exchange with a peer (stateless!)
//! request_key_exchange(
//!     &network_client,
//!     &context,
//!     our_identity,
//!     peer_id,
//!     &context_client,
//!     Duration::from_secs(10),
//! ).await?;
//! ```
//!
//! # Testing
//!
//! All protocols have comprehensive test coverage:
//! - 24 unit tests validating protocol logic
//! - MockDeltaStore for testing DAG operations
//! - Crypto validation tests (encryption, signatures, nonces)
//!
//! See `tests/` directory for examples.

#![warn(missing_docs)]

pub mod gossipsub;
pub mod p2p;
pub mod stream;

// Re-export commonly used types
pub use stream::SecureStream;

