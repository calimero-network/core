//! Calimero Network Protocols
//!
//! Stateless protocol handlers for Calimero node communication.
//!
//! # Architecture
//!
//! This crate provides pure, stateless protocol handlers that can be tested in isolation.
//! All state is injected as parameters - no hidden dependencies, no actors, no framework magic.
//!
//! ## Modules
//!
//! - `gossipsub`: Broadcast protocols (one-to-many, public)
//! - `p2p`: Request/response protocols (one-to-one, authenticated)
//! - `stream`: Authenticated stream helpers (always secure)
//!
//! ## Design Principles
//!
//! 1. **Stateless**: All handlers are pure functions (state injected as params)
//! 2. **Testable**: No dependencies on full node infrastructure
//! 3. **Reusable**: Can be used in different contexts (node, tests, tools)
//! 4. **Secure by default**: All P2P uses authenticated streams
//! 5. **No actors**: Plain async Rust (no framework coupling)

pub mod gossipsub;
pub mod p2p;
pub mod stream;

// Re-export commonly used types
pub use stream::SecureStream;

