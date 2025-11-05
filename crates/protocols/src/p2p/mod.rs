//! P2P Request/Response Protocols
//!
//! Handlers for one-to-one request/response patterns over P2P streams.
//!
//! These protocols are PRIVATE (require authentication) and use
//! authenticated streams to verify peer membership and encrypt data.

pub mod blob_request;
pub mod delta_request;
pub mod key_exchange;

