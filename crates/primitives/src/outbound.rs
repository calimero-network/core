//! The wire between a run and the node for an outbound HTTPS request.
//!
//! A run may ask its node for HTTPS requests to an outside service (at most
//! the component host's `MAX_HTTP_REQUESTS` in one run), for what an app
//! cannot know by itself: whether a captcha was solved, whether a payment went
//! through, what an API answers. The component host passes the request
//! (`wasi:http`, `calimero_app_host::HttpHost`) to the node, which encodes it
//! as an [`OutboundRequest`] for `calimero-outbound`.
//! The request is made only if every gate holds, and refused otherwise:
//!
//! - the node's operator turned outbound requests on (`[outbound]`, off by
//!   default) and allows the request's host (listed, or `"*"` for any host an
//!   app declares);
//! - the app's signed release manifest declares a URL prefix that covers the
//!   request's URL (`outbound`);
//! - a prefix that covers it is approved for the app's publisher, by whoever
//!   holds outbound approvals: the app's organization (`MANAGE_APPLICATION`
//!   org-wide, `POST /orgs/{org}/outbound`) or the operator
//!   (`--outbound-approvals operator`) ([`OutboundSelection`]);
//! - the run writes (a call, a follow-up, a timer, a delivery) or is a
//!   component's action: never a view, `init`, a migration or compute;
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
//! part of a log entry. Only the state the run writes because of them is.

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
/// holds its app's proposal lock meanwhile.
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

/// What starts a reference to a sealed secret in a header value or the body:
/// `{{sealed:<hex envelope>}}`. See [`SealedSecret`].
pub const SEALED_PREFIX: &str = "{{sealed:";

/// The largest sealed envelope a request may name, in bytes.
pub const MAX_SEALED_ENVELOPE: usize = 4 * 1024;

/// What an app seals for the node to put in an outbound request: the value,
/// and the only hosts it may ever be sent to. Sealed together, so the app
/// cannot send the value anywhere the admin did not name when sealing it.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SealedSecretPayload {
    pub value: String,
    pub hosts: Vec<String>,
}

/// A secret sealed to the namespace TEE key, which an app keeps in its state.
///
/// Every member replicates it; none can read it. A request names it with
/// [`placeholder`](Self::placeholder), and only a node that is a TEE authority
/// of the namespace opens it, fills it in for a host it names, and removes it
/// from the response, so the app's code never holds the value. On any other
/// node the request is refused `SealedUnavailable`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(feature = "borsh", derive(BorshDeserialize, BorshSerialize))]
pub struct SealedSecret {
    /// The namespace TEE key it is sealed to. When the namespace's key
    /// rotates it no longer is the current one, and the app seals it again.
    pub key: [u8; 32],
    /// The [`SealedSecretPayload`], sealed in the app's context.
    pub envelope: Vec<u8>,
}

impl SealedSecret {
    /// What to write in a header value or the body where the value goes.
    #[must_use]
    pub fn placeholder(&self) -> String {
        format!(
            "{SEALED_PREFIX}{}{SECRET_SUFFIX}",
            hex::encode(&self.envelope)
        )
    }
}

/// A refusal an app can act on, read from the reason an outbound request
/// returns. Every other refusal (a host the operator does not allow, a URL
/// outside the manifest, a bound) is a bug in the app or its deployment, and
/// reads as `None`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutboundRefusal {
    /// Whoever holds outbound approvals has not approved the URL's prefix for
    /// the app's publisher, or revoked it: a member should ask an admin.
    NotApproved,
    /// A newer version of the app declares the prefix and nobody has been
    /// asked about it yet: an admin should approve it from the upgrade.
    AwaitingApproval,
    /// The request names a [`SealedSecret`] and this node is not a TEE
    /// authority of the namespace, so it cannot open it: the request has to
    /// run on one of the namespace's TEE nodes.
    SealedUnavailable,
}

impl OutboundRefusal {
    /// The word a refusal's reason starts with. Stable: the node writes it and
    /// apps match on it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::NotApproved => "NotApproved",
            Self::AwaitingApproval => "AwaitingApproval",
            Self::SealedUnavailable => "SealedUnavailable",
        }
    }

    /// The refusal a reason names, if it is one an app can act on.
    #[must_use]
    pub fn of(reason: &str) -> Option<Self> {
        [
            Self::NotApproved,
            Self::AwaitingApproval,
            Self::SealedUnavailable,
        ]
        .into_iter()
        .find(|refusal| {
            reason
                .strip_prefix(refusal.word())
                .is_some_and(|rest| rest.starts_with(':'))
        })
    }
}

/// Which URL prefixes an approver approves for an app's publisher: exactly the
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
    use super::{OutboundRefusal, OutboundSelection};

    #[test]
    fn a_reason_names_the_refusal_an_app_can_act_on() {
        assert_eq!(
            OutboundRefusal::of(
                "NotApproved: the namespace has not approved https://a/ for this app"
            ),
            Some(OutboundRefusal::NotApproved)
        );
        assert_eq!(
            OutboundRefusal::of("AwaitingApproval: https://a/ is a URL prefix this app declares"),
            Some(OutboundRefusal::AwaitingApproval)
        );
        assert_eq!(
            OutboundRefusal::of("no URL prefix the app declares covers it"),
            None
        );
        assert_eq!(
            OutboundRefusal::of("SealedUnavailable: this node is not a TEE of the namespace"),
            Some(OutboundRefusal::SealedUnavailable)
        );
        assert_eq!(OutboundRefusal::of("NotApprovedYet: something else"), None);
    }

    #[test]
    fn a_sealed_secret_is_named_by_its_envelope_in_hex() {
        let sealed = super::SealedSecret {
            key: [7; 32],
            envelope: vec![0xab, 0x01],
        };
        assert_eq!(sealed.placeholder(), "{{sealed:ab01}}");
    }

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
