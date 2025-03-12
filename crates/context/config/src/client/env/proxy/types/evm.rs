use alloy_sol_types::{sol, SolValue};
use alloy::primitives::{Address, Bytes, U256};

use crate::{repr::Repr, types::{Identity, ProposalId, SignerId}, Proposal, ProposalAction};

sol! {
    // Data structures
    enum SolProposalActionKind {
      ExternalFunctionCall,
      Transfer,
      SetNumApprovals,
      SetActiveProposalsLimit,
      SetContextValue,
      DeleteProposal
    }

    struct SolProposalAction {
      SolProposalActionKind kind;
      bytes data;
    }

    struct SolProposal {
      bytes32 id;
      bytes32 authorId;
      SolProposalAction[] actions;
    }

    struct ExternalFunctionCallData {
        address target;
        bytes callData;
        uint256 value;
    }

    struct TransferData {
        address recipient;
        uint256 amount;
    }

    struct ContextValueData {
        bytes key;
        bytes value;
    }
}

// Add conversions from Sol types to our domain types
impl From<SolProposal> for Proposal {
    fn from(sol_proposal: SolProposal) -> Self {
        Proposal {
            id: Repr::new(ProposalId(Identity(sol_proposal.id.into()))),
            author_id: Repr::new(SignerId(Identity(sol_proposal.authorId.into()))),
            actions: sol_proposal.actions.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<SolProposalAction> for ProposalAction {
    fn from(action: SolProposalAction) -> Self {
        match action.kind {
            SolProposalActionKind::ExternalFunctionCall => {
                let data: ExternalFunctionCallData = 
                    SolValue::abi_decode(&action.data, false)
                        .expect("Invalid external call data");
                ProposalAction::ExternalFunctionCall {
                    receiver_id: format!("{:?}", data.target),
                    method_name: String::from_utf8(data.callData[..4].to_vec())
                        .expect("Invalid method name"),
                    args: String::from_utf8(data.callData[4..].to_vec())
                        .expect("Invalid args"),
                    deposit: data.value.try_into()
                        .expect("Amount too large for native token"),
                }
            },
            SolProposalActionKind::Transfer => {
                let data: TransferData = 
                    SolValue::abi_decode(&action.data, false)
                        .expect("Invalid transfer data");
                ProposalAction::Transfer {
                    receiver_id: format!("{:?}", data.recipient),
                    amount: data.amount.try_into()
                        .expect("Amount too large for native token"),
                }
            },
            SolProposalActionKind::SetNumApprovals => {
                let num_approvals: u32 = 
                    SolValue::abi_decode(&action.data, false)
                        .expect("Invalid num approvals data");
                ProposalAction::SetNumApprovals { num_approvals }
            },
            SolProposalActionKind::SetActiveProposalsLimit => {
                let active_proposals_limit: u32 = 
                    SolValue::abi_decode(&action.data, false)
                        .expect("Invalid proposals limit data");
                ProposalAction::SetActiveProposalsLimit { active_proposals_limit }
            },
            SolProposalActionKind::SetContextValue => {
                let data: ContextValueData = 
                    SolValue::abi_decode(&action.data, false)
                        .expect("Invalid context value data");
                ProposalAction::SetContextValue {
                    key: data.key.to_vec().into_boxed_slice(),
                    value: data.value.to_vec().into_boxed_slice(),
                }
            },
            SolProposalActionKind::DeleteProposal => {
                let proposal_id: [u8; 32] = 
                    SolValue::abi_decode(&action.data, false)
                        .expect("Invalid proposal id data");
                ProposalAction::DeleteProposal {
                    proposal_id: Repr::new(ProposalId(Identity(proposal_id))),
                }
            },
            SolProposalActionKind::__Invalid => {
                panic!("Invalid proposal action kind encountered in response")
            }
        }
    }
}

// We'll need to define this enum to match the Solidity contract's action data structures
#[derive(Debug)]
pub enum ProposalActionData {
    ExternalFunctionCall {
        target: Address,
        call_data: Vec<u8>,
        value: u128,
    },
    Transfer {
        recipient: Address,
        amount: u128,
    },
    SetNumApprovals(u32),
    SetActiveProposalsLimit(u32),
    SetContextValue {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    DeleteProposal(ProposalId),
}