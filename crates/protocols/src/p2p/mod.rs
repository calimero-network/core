//! P2P Request/Response Protocols
//!
//! Handlers for one-to-one request/response patterns over P2P streams.
//!
//! Protocols:
//! - `blob_protocol` - PUBLIC blob download (CALIMERO_BLOB_PROTOCOL)
//! - `blob_request` - PRIVATE blob sharing (context-authenticated)
//! - `delta_request` - PRIVATE delta request/response
//! - `key_exchange` - PRIVATE key exchange

pub mod blob_protocol;
pub mod blob_request;
pub mod delta_request;
pub mod key_exchange;

