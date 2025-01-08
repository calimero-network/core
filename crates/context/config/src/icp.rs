use candid::{CandidType, Principal};
use serde::Deserialize;

pub mod repr;
pub mod types;

use repr::ICRepr;

use crate::repr::ReprTransmute;
use crate::types::{ProposalId, SignerId};
use crate::{Proposal, ProposalAction, ProposalWithApprovals, ProxyMutateRequest};

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
    DeleteProposal {
        proposal_id: ICRepr<ProposalId>,
    },
}

impl TryFrom<ProposalAction> for ICProposalAction {
    type Error = String;

    fn try_from(action: ProposalAction) -> Result<Self, Self::Error> {
        let action = match action {
            ProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
            } => ICProposalAction::ExternalFunctionCall {
                receiver_id: receiver_id
                    .parse::<Principal>()
                    .map_err(|e| e.to_string())?,
                method_name,
                args,
                deposit,
            },
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => ICProposalAction::Transfer {
                receiver_id: receiver_id
                    .parse::<Principal>()
                    .map_err(|e| e.to_string())?,
                amount,
            },
            ProposalAction::SetNumApprovals { num_approvals } => {
                ICProposalAction::SetNumApprovals { num_approvals }
            }
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => ICProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            },
            ProposalAction::SetContextValue { key, value } => ICProposalAction::SetContextValue {
                key: key.into(),
                value: value.into(),
            },
            ProposalAction::DeleteProposal { proposal_id } => ICProposalAction::DeleteProposal {
                proposal_id: proposal_id.rt().map_err(|e| e.to_string())?,
            },
        };

        Ok(action)
    }
}

impl From<ICProposalAction> for ProposalAction {
    fn from(action: ICProposalAction) -> Self {
        match action {
            ICProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
            } => ProposalAction::ExternalFunctionCall {
                receiver_id: receiver_id.to_text(),
                method_name,
                args,
                deposit,
            },
            ICProposalAction::Transfer {
                receiver_id,
                amount,
            } => ProposalAction::Transfer {
                receiver_id: receiver_id.to_text(),
                amount,
            },
            ICProposalAction::SetNumApprovals { num_approvals } => {
                ProposalAction::SetNumApprovals { num_approvals }
            }
            ICProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            },
            ICProposalAction::SetContextValue { key, value } => ProposalAction::SetContextValue {
                key: key.into_boxed_slice(),
                value: value.into_boxed_slice(),
            },
            ICProposalAction::DeleteProposal { proposal_id } => ProposalAction::DeleteProposal {
                proposal_id: proposal_id.rt().expect("infallible conversion"),
            },
        }
    }
}

#[derive(CandidType, Deserialize, Clone, Debug, PartialEq)]
pub struct ICProposal {
    pub id: ICRepr<ProposalId>,
    pub author_id: ICRepr<SignerId>,
    pub actions: Vec<ICProposalAction>,
}

impl From<ICProposal> for Proposal {
    fn from(proposal: ICProposal) -> Self {
        Proposal {
            id: proposal.id.rt().expect("infallible conversion"),
            author_id: proposal.author_id.rt().expect("infallible conversion"),
            actions: proposal.actions.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(CandidType, Deserialize, Copy, Clone, Debug)]
pub struct ICProposalWithApprovals {
    pub proposal_id: ICRepr<ProposalId>,
    pub num_approvals: usize,
}

impl From<ICProposalWithApprovals> for ProposalWithApprovals {
    fn from(proposal: ICProposalWithApprovals) -> Self {
        ProposalWithApprovals {
            proposal_id: proposal.proposal_id.rt().expect("infallible conversion"),
            num_approvals: proposal.num_approvals,
        }
    }
}

#[derive(CandidType, Deserialize, Copy, Clone, Debug)]
pub struct ICProposalApprovalWithSigner {
    pub proposal_id: ICRepr<ProposalId>,
    pub signer_id: ICRepr<SignerId>,
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

impl TryFrom<ProxyMutateRequest> for ICProxyMutateRequest {
    type Error = String;

    fn try_from(request: ProxyMutateRequest) -> Result<Self, Self::Error> {
        let request = match request {
            ProxyMutateRequest::Propose { proposal } => ICProxyMutateRequest::Propose {
                proposal: ICProposal {
                    id: proposal.id.rt().map_err(|e| e.to_string())?,
                    author_id: proposal.author_id.rt().map_err(|e| e.to_string())?,
                    actions: proposal
                        .actions
                        .into_iter()
                        .map(TryInto::try_into)
                        .collect::<Result<_, _>>()?,
                },
            },
            ProxyMutateRequest::Approve { approval } => ICProxyMutateRequest::Approve {
                approval: ICProposalApprovalWithSigner {
                    proposal_id: approval.proposal_id.rt().map_err(|e| e.to_string())?,
                    signer_id: approval.signer_id.rt().map_err(|e| e.to_string())?,
                },
            },
        };

        Ok(request)
    }
}
