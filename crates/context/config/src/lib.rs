#![allow(single_use_lifetimes)]

use std::borrow::Cow;
use std::time;

use serde::{Deserialize, Serialize};

pub mod repr;
pub mod types;

use repr::Repr;
use types::{Application, Capability, ContextId, ContextIdentity, SignerId};

pub type Timestamp = u64;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct Request<'a> {
    #[serde(borrow, flatten)]
    pub kind: RequestKind<'a>,

    pub signer_id: Repr<SignerId>,
    pub timestamp_ms: Timestamp,
}

impl<'a> Request<'a> {
    pub fn new(signer_id: SignerId, kind: RequestKind<'a>) -> Self {
        let timestamp_ms = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .expect("system time is before epoch?")
            .as_millis() as _;

        Request {
            signer_id: Repr::new(signer_id),
            timestamp_ms,
            kind,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
pub enum RequestKind<'a> {
    #[serde(borrow)]
    Context(ContextRequest<'a>),
    System(SystemRequest),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ContextRequest<'a> {
    pub context_id: Repr<ContextId>,

    #[serde(borrow, flatten)]
    pub kind: ContextRequestKind<'a>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
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
pub enum SystemRequest {
    #[serde(rename_all = "camelCase")]
    SetValidityThreshold { threshold_ms: Timestamp },
}
