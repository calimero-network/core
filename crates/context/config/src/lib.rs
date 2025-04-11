#![allow(single_use_lifetimes, reason = "False positive")]

use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "icp")]
pub mod icp;
pub mod repr;
#[cfg(feature = "stellar")]
pub mod stellar;
pub mod types;

use repr::Repr;
use types::{Application, Capability, ContextId, ContextIdentity, ProposalId, SignerId};

pub type Timestamp = u64;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Request<'a> {
    pub signer_id: Repr<SignerId>,
    pub nonce: u64,

    #[serde(borrow, flatten)]
    pub kind: RequestKind<'a>,
}

impl<'a> Request<'a> {
    #[must_use]
    pub fn new(signer_id: SignerId, kind: RequestKind<'a>, nonce: u64) -> Self {
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
    UpdateProxyContract,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum SystemRequest {
    #[serde(rename_all = "camelCase")]
    SetValidityThreshold { threshold_ms: Timestamp },
}

/// Proxy contract
/// TODO: move these to a separate cratexs
pub type Gas = u64;
pub type NativeToken = u128;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BorshDeserialize,
    BorshSerialize,
    Ord,
    PartialOrd,
    Eq,
)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum ProposalAction {
    ExternalFunctionCall {
        receiver_id: String,
        method_name: String,
        args: String,
        deposit: NativeToken,
    },
    Transfer {
        receiver_id: String,
        amount: NativeToken,
    },
    SetNumApprovals {
        num_approvals: u32,
    },
    SetActiveProposalsLimit {
        active_proposals_limit: u32,
    },
    SetContextValue {
        key: Box<[u8]>,
        value: Box<[u8]>,
    },
    DeleteProposal {
        proposal_id: Repr<ProposalId>,
    },
}

// The proposal the user makes specifying the receiving account and actions they want to execute (1 tx)
#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    BorshDeserialize,
    BorshSerialize,
    Ord,
    PartialOrd,
    Eq,
)]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub struct Proposal {
    pub id: Repr<ProposalId>,
    pub author_id: Repr<SignerId>,
    pub actions: Vec<ProposalAction>,
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalApprovalWithSigner {
    pub proposal_id: Repr<ProposalId>,
    pub signer_id: Repr<SignerId>,
    pub added_timestamp: u64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "scope", content = "params")]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum ProxyMutateRequest {
    Propose {
        proposal: Proposal,
    },
    Approve {
        approval: ProposalApprovalWithSigner,
    },
}

#[derive(PartialEq, Serialize, Deserialize, Copy, Clone, Debug)]
#[serde(deny_unknown_fields)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub struct ProposalWithApprovals {
    pub proposal_id: Repr<ProposalId>,
    pub num_approvals: usize,
}
