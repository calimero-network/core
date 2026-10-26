//! The wire between an app and the node for an outbound HTTPS request.
//!
//! A run may ask its node to make **one** HTTPS request to an outside
//! service, for what a contract cannot know by itself: whether a captcha was
//! solved, whether a payment went through, what an API answers. The node makes it
//! only if every gate holds, and refuses otherwise (see the runtime's
//! `http_request` host function and `calimero-context`'s outbound policy):
//!
//! - the node's operator turned outbound requests on (`[context.outbound]`,
//!   off by default) and allows the request's host (listed, or `"*"` for any
//!   host an app declares);
//! - the app's signed bundle manifest declares a URL prefix that covers the
//!   request's URL (`outbound`);
//! - the namespace the run belongs to approved a prefix that covers it for the
//!   app's publisher (`GroupOp::OutboundApprovalSet`), which an admin does in
//!   the same call that installs or upgrades the app ([`OutboundSelection`]);
//! - the run writes state and executes on this node (its own write, or one it
//!   relays for an author): never a view, a migration, a check or a peer's
//!   delta;
//! - the request fits the limits below, and its host resolves only to public
//!   addresses.
//!
//! Secrets the app needs (an API key) are the operator's, never the app's: the
//! request names one as `{{secret:NAME}}` in a header value or the body, and
//! the node substitutes it only into a request to the host the operator bound
//! that secret to. The app never reads it, and any copy of it in the response
//! is removed before the app sees the response.
//!
//! Like search, this is node-local: the request and the response are never
//! part of a delta. Only the state the run writes because of them is.

#[cfg(feature = "borsh")]
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

/// Requests one run may make. One: enough to ask a service one question, and
/// no loop a run could spend sending state out piece by piece.
pub const MAX_REQUESTS_PER_RUN: u64 = 1;

/// Largest request body, in bytes.
pub const MAX_REQUEST_BODY: usize = 64 * 1024;

/// Largest URL, in bytes.
pub const MAX_URL: usize = 2048;

/// Headers a request may set.
pub const MAX_HEADERS: usize = 8;

/// Largest header name plus value, in bytes.
pub const MAX_HEADER_BYTES: usize = 1024;

/// Largest response body the node reads, in bytes. A longer one is refused,
/// not truncated, so an app never acts on half an answer.
pub const MAX_RESPONSE_BODY: usize = 1024 * 1024;

/// How long the node waits for the whole request, in milliseconds. The run
/// holds its context's write lock meanwhile.
pub const TIMEOUT_MS: u64 = 10_000;

/// How a request names a secret the node fills in: `{{secret:NAME}}`.
pub const SECRET_PREFIX: &str = "{{secret:";

/// The end of a secret placeholder.
pub const SECRET_SUFFIX: &str = "}}";

/// The methods a request may use. Append only: the borsh tag is the index.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub enum OutboundMethod {
    /// `GET`, with no body.
    Get,
    /// `POST`, with the body as given.
    Post,
    /// `PUT`, with the body as given.
    Put,
    /// `PATCH`, with the body as given.
    Patch,
    /// `DELETE`, with the body as given (usually empty).
    Delete,
}

/// What an app asks its node to send.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct OutboundRequest {
    pub method: OutboundMethod,
    /// The URL, under a prefix the app's manifest declares. `https` only; a
    /// query string is allowed, a fragment is not.
    pub url: String,
    /// Header names and values. `Host`, `Connection`, `Content-Length`,
    /// `Transfer-Encoding`, `Proxy-*` and `Cookie` are refused; the node sets
    /// what it must itself.
    pub headers: Vec<(String, String)>,
    /// The body; empty for a `GET`.
    pub body: Vec<u8>,
}

/// What the node hands back: the status and the body, never the response's
/// headers, cookies or the address it reached.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct OutboundResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Which URL prefixes an admin approves for an app's publisher: exactly the
/// ones its manifest declares, or a list of their own (what an install screen
/// showed, a subset of it, or a narrower prefix).
///
/// On the wire, the string `"manifest"` or an array of prefixes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundSelection {
    /// Every prefix the app's manifest declares, read by the node.
    Manifest,
    /// These prefixes.
    Prefixes(Vec<String>),
}

impl Serialize for OutboundSelection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Manifest => serializer.serialize_str("manifest"),
            Self::Prefixes(prefixes) => serde::Serialize::serialize(prefixes, serializer),
        }
    }
}

impl<'de> Deserialize<'de> for OutboundSelection {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Wire {
            Keyword(String),
            Prefixes(Vec<String>),
        }
        match Wire::deserialize(deserializer)? {
            Wire::Keyword(word) if word == "manifest" => Ok(Self::Manifest),
            Wire::Keyword(word) => Err(serde::de::Error::custom(format!(
                "expected \"manifest\" or a list of URL prefixes, got {word:?}"
            ))),
            Wire::Prefixes(prefixes) => Ok(Self::Prefixes(prefixes)),
        }
    }
}

#[cfg(test)]
mod selection_tests {
    use super::OutboundSelection;

    #[test]
    fn a_selection_is_the_manifest_keyword_or_a_list() {
        let manifest: OutboundSelection = serde_json::from_str(r#""manifest""#).unwrap();
        assert_eq!(manifest, OutboundSelection::Manifest);
        let list: OutboundSelection =
            serde_json::from_str(r#"["https://api.example/v1/"]"#).unwrap();
        assert_eq!(
            list,
            OutboundSelection::Prefixes(vec!["https://api.example/v1/".to_owned()])
        );
        assert!(serde_json::from_str::<OutboundSelection>(r#""all""#).is_err());
        for value in [manifest, list] {
            let back: OutboundSelection =
                serde_json::from_str(&serde_json::to_string(&value).unwrap()).unwrap();
            assert_eq!(back, value);
        }
    }
}
