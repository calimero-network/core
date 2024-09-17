#![allow(single_use_lifetimes, reason = "False positive")]

use std::borrow::Cow;
use std::time;

use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
pub mod client;
pub mod repr;
pub mod types;

use repr::Repr;
use types::{Application, Capability, ContextId, ContextIdentity, SignerId};

pub type Timestamp = u64;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Request<'a> {
    #[serde(borrow, flatten)]
    pub kind: RequestKind<'a>,

    pub signer_id: Repr<SignerId>,
    pub timestamp_ms: Timestamp,
}

impl<'a> Request<'a> {
    #[must_use]
    pub fn new(signer_id: SignerId, kind: RequestKind<'a>) -> Self {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "This is never expected to overflow"
        )]
        let timestamp_ms = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .expect("system time is before epoch?")
            .as_millis() as u64;

        Request {
            signer_id: Repr::new(signer_id),
            timestamp_ms,
            kind,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum RequestKind<'a> {
    #[serde(borrow)]
    Context(ContextRequest<'a>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ContextRequest<'a> {
    pub context_id: Repr<ContextId>,

    #[serde(borrow, flatten)]
    pub kind: ContextRequestKind<'a>,
}

impl<'a> ContextRequest<'a> {
    #[must_use]
    pub const fn new(context_id: Repr<ContextId>, kind: ContextRequestKind<'a>) -> Self {
        ContextRequest { context_id, kind }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum ContextRequestKind<'a> {
    Add {
        author_id: Repr<ContextIdentity>,
        #[serde(borrow)]
        application: Application<'a>,
    },
    UpdateApplication {
        #[serde(borrow)]
        application: Application<'a>,
    },
    AddMembers {
        members: Cow<'a, [Repr<ContextIdentity>]>,
    },
    RemoveMembers {
        members: Cow<'a, [Repr<ContextIdentity>]>,
    },
    Grant {
        capabilities: Cow<'a, [(Repr<ContextIdentity>, Capability)]>,
    },
    Revoke {
        capabilities: Cow<'a, [(Repr<ContextIdentity>, Capability)]>,
    },
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum SystemRequest {
    #[serde(rename_all = "camelCase")]
    SetValidityThreshold { threshold_ms: Timestamp },
}
