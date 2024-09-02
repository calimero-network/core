#![allow(
    unused_results,
    single_use_lifetimes,
    variant_size_differences,
    unused_crate_dependencies
)]

use std::borrow::Cow;
use std::time;

use near_sdk::{near, Timestamp};

mod app;
pub mod repr;
pub mod types;

use repr::Repr;
use types::{Application, Capability, ContextId, ContextIdentity, SignerId};

#[doc(hidden)]
pub mod __private {
    pub use super::app::ContextConfig as near;
}

#[derive(Debug)]
#[near(serializers = [json])]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct Request<'a> {
    #[serde(borrow, flatten)]
    pub kind: RequestKind<'a>,

    signer_id: Repr<SignerId>,
    timestamp_ms: Timestamp,
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

#[derive(Debug)]
#[near(serializers = [json])]
#[serde(tag = "scope", content = "params")]
pub enum RequestKind<'a> {
    #[serde(borrow)]
    Context(ContextRequest<'a>),
    System(SystemRequest),
}

#[derive(Debug)]
#[near(serializers = [json])]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct ContextRequest<'a> {
    pub context_id: Repr<ContextId>,

    #[serde(borrow, flatten)]
    pub kind: ContextRequestKind<'a>,
}

#[derive(Debug)]
#[near(serializers = [json])]
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

#[derive(Copy, Clone, Debug)]
#[near(serializers = [json])]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
pub enum SystemRequest {
    SetValidityThreshold { threshold_ms: Timestamp },
}
