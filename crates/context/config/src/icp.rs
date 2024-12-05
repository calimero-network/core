use candid::{CandidType, Principal};
use serde::Deserialize;

pub mod repr;
pub mod types;

use repr::ICRepr;

use crate::types::{ProposalId, SignerId};

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
pub enum ICProposalAction {
    ExternalFunctionCall {
        receiver_id: Principal,
        method_name: String,
        args: String,
        deposit: u128,
    },
    Transfer {
        receiver_id: Principal,
        amount: u128,
    },
    SetNumApprovals {
        num_approvals: u32,
    },
    SetActiveProposalsLimit {
        active_proposals_limit: u32,
    },
    SetContextValue {
        key: Vec<u8>,
        value: Vec<u8>,
    },
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
pub struct ICProposal {
    pub id: ICRepr<ProposalId>,
    pub author_id: ICRepr<SignerId>,
    pub actions: Vec<ICProposalAction>,
}

#[derive(CandidType, Deserialize, Copy, Clone, Debug)]
pub struct ICProposalWithApprovals {
    pub proposal_id: ICRepr<ProposalId>,
    pub num_approvals: usize,
}

#[derive(CandidType, Deserialize, Copy, Clone, Debug)]
pub struct ICProposalApprovalWithSigner {
    pub proposal_id: ICRepr<ProposalId>,
    pub signer_id: ICRepr<SignerId>,
    pub added_timestamp: u64,
}

#[derive(CandidType, Deserialize, Clone, Debug)]
pub enum ICProxyMutateRequest {
    Propose {
        proposal: ICProposal,
    },
    Approve {
        approval: ICProposalApprovalWithSigner,
    },
}
