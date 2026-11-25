//! Types shared across the workspace with no other Calimero dependency: the 32-byte ids
//! (`ApplicationId`, `BlobId`, `ContextId`, over [`hash::Hash`]), the outbound HTTPS wire
//! ([`outbound`]), the search vocabulary ([`search`]) and [`utils::loggable_url`].
//!
//! Spec: [Glossary](https://calimero-network.github.io/core/protocol/glossary/).
//! Crate guide: [`crates/primitives/AGENTS.md`](https://github.com/calimero-network/core/blob/master/crates/primitives/AGENTS.md).

pub mod application;
pub mod blobs;
pub mod common;
pub mod context;
#[cfg(test)]
#[path = "tests/encoding.rs"]
mod encoding_tests;
pub mod hash;
pub mod outbound;
pub mod search;
pub mod utils;
