//! Network protocol handlers for P2P and broadcast communication.
//!
//! Provides stateless protocol implementations that can be tested independently.
//! All dependencies are injected as function parameters.
//!
//! # Modules
//!
//! - [`gossipsub`]: Broadcast protocols (state deltas)
//! - [`p2p`]: Request/response protocols (key exchange, delta/blob requests)
//! - [`stream`]: Authenticated stream utilities
//!
//! # Example
//!
//! ```rust,ignore
//! use calimero_protocols::p2p::key_exchange;
//!
//! key_exchange::request_key_exchange(
//!     &network_client,
//!     &context,
//!     our_identity,
//!     peer_id,
//!     &context_client,
//!     timeout,
//! ).await?;
//! ```

#![warn(missing_docs)]

pub mod gossipsub;
pub mod p2p;
pub mod stream;

pub use stream::SecureStream;
