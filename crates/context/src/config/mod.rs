//! Configuration management for Calimero contexts - flattened structure

pub mod operations;
pub mod protocols;
pub mod environment;

use serde::{Deserialize, Serialize};
use crate::types::{ContextId, SignerId};
use crate::repr::Repr;

pub use operations::*;
pub use protocols::*;
pub use environment::*;

/// Utility function to humanize iterator output
pub fn humanize_iter<I>(iter: I) -> String
where
    I: IntoIterator,
    I::Item: std::fmt::Display,
{
    let items: Vec<String> = iter.into_iter().map(|item| item.to_string()).collect();
    items.join(", ")
}

pub type Timestamp = u64;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Request {
    pub signer_id: Repr<SignerId>,
    pub nonce: u64,
    #[serde(flatten)]
    pub kind: RequestKind,
}

impl Request {
    #[must_use]
    pub fn new(signer_id: SignerId, kind: RequestKind, nonce: u64) -> Self {
        Request {
            signer_id: Repr::new(signer_id),
            kind,
            nonce,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum RequestKind {
    Context(ContextRequest),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextRequest {
    pub context_id: Repr<ContextId>,
    #[serde(flatten)]
    pub kind: ContextRequestKind,
}

impl ContextRequest {
    #[must_use]
    pub const fn new(context_id: Repr<ContextId>, kind: ContextRequestKind) -> Self {
        ContextRequest { context_id, kind }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum ContextRequestKind {
    // Add other request kinds as needed
}