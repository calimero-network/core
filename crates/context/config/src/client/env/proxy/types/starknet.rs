use starknet::core::types::Felt;
use starknet::core::codec::{Encode, Decode};
use starknet::core::types::U256;

use crate::repr::{Repr, ReprBytes};
use crate::types::{ContextIdentity, ProposalId, SignerId};
use crate::{Proposal, ProposalAction, ProposalApprovalWithSigner, ProposalWithApprovals, ProxyMutateRequest};

#[derive(Debug, Encode, Decode)]
pub struct StarknetProposalId {
    pub high: Felt,
    pub low: Felt,
}

impl From<Repr<ProposalId>> for StarknetProposalId {
  fn from(value: Repr<ProposalId>) -> Self {
      let bytes = value.as_bytes();
      let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
      StarknetProposalId {
          high: Felt::from_bytes_be_slice(high_bytes),
          low: Felt::from_bytes_be_slice(low_bytes),
      }
  }
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetIdentity {
    pub high: Felt,
    pub low: Felt,
}

impl From<Repr<SignerId>> for StarknetIdentity {
    fn from(value: Repr<SignerId>) -> Self {
        let bytes = value.as_bytes();
        let (high_bytes, low_bytes) = bytes.split_at(bytes.len() / 2);
        StarknetIdentity {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        }
    }
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetProxyMutateRequest {
    pub signer_id: StarknetIdentity,
    pub kind: StarknetProxyMutateRequestKind,
}

#[derive(Debug, Encode, Decode)]
pub enum StarknetProxyMutateRequestKind {
    Propose(StarknetProposal),
    Approve(StarknetConfirmationRequest),
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetProposal {
    pub proposal_id: StarknetProposalId,
    pub author_id: StarknetIdentity,
    pub actions: StarknetProposalActionWithArgs,
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetConfirmationRequest {
    pub proposal_id: StarknetProposalId,
    pub signer_id: StarknetIdentity,
    pub added_timestamp: Felt, // u64 in contract
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetU256 {
    pub high: Felt,
    pub low: Felt,
}

impl From<U256> for StarknetU256 {
    fn from(value: U256) -> Self {
        StarknetU256 {
            high: Felt::from(value.high()),  // Get high 128 bits
            low: Felt::from(value.low())     // Get low 128 bits
        }
    }
}

impl From<u128> for StarknetU256 {
    fn from(value: u128) -> Self {
        StarknetU256 {
            high: Felt::ZERO,
            low: Felt::from(value)
        }
    }
}

#[derive(Debug, Encode, Decode)]
pub enum StarknetProposalActionWithArgs {
    ExternalFunctionCall(Felt, Felt, Vec<Felt>),
    Transfer(Felt, StarknetU256, Felt),
    SetNumApprovals(Felt),
    SetActiveProposalsLimit(Felt),
    SetContextValue(Vec<Felt>, Vec<Felt>)
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetSignedRequest {
    pub payload: Vec<Felt>,
    pub signature_r: Felt,
    pub signature_s: Felt,
}

impl From<ProxyMutateRequest> for StarknetProxyMutateRequestKind {
    fn from(request: ProxyMutateRequest) -> Self {
        match request {
            ProxyMutateRequest::Propose { proposal} => {
                StarknetProxyMutateRequestKind::Propose(proposal.into())
            },
            ProxyMutateRequest::Approve { approval } => {
                StarknetProxyMutateRequestKind::Approve(approval.into())
            },
        }
    }
}

impl From<Proposal> for StarknetProposal {
  fn from(proposal: Proposal) -> Self {
      StarknetProposal {
          proposal_id: proposal.id.into(),
          author_id: proposal.author_id.into(),
          actions: proposal.actions.into(),
      }
  }
}

impl From<ProposalApprovalWithSigner> for StarknetConfirmationRequest {
  fn from(approval: ProposalApprovalWithSigner) -> Self {
      StarknetConfirmationRequest {
          proposal_id: approval.proposal_id.into(),
          signer_id: approval.signer_id.into(),
          added_timestamp: Felt::from(approval.added_timestamp),
      }
  }
}

impl From<(Repr<SignerId>, ProxyMutateRequest)> for StarknetProxyMutateRequest {
    fn from((signer_id, request): (Repr<SignerId>, ProxyMutateRequest)) -> Self {
        StarknetProxyMutateRequest {
            signer_id: signer_id.into(),
            kind: request.into(),
        }
    }
}

impl From<Vec<ProposalAction>> for StarknetProposalActionWithArgs {
    fn from(actions: Vec<ProposalAction>) -> Self {
        let action = actions.into_iter().next().expect("At least one action required");
        
        match action {
            ProposalAction::ExternalFunctionCall { 
                receiver_id, 
                method_name, 
                args, 
                deposit, 
                gas 
            } => {
                StarknetProposalActionWithArgs::ExternalFunctionCall(
                    Felt::from_bytes_be_slice(receiver_id.as_bytes()),
                    Felt::from_bytes_be_slice(method_name.as_bytes()),
                    vec![] // TODO: parse args string into Felts
                )
            },
            ProposalAction::Transfer { 
                receiver_id, 
                amount 
            } => {
                StarknetProposalActionWithArgs::Transfer(
                    Felt::from_bytes_be_slice(receiver_id.as_bytes()),
                    amount.into(),  // converts to StarknetU256
                    Felt::ZERO  // TODO: determine correct token address
                )
            },
            ProposalAction::SetNumApprovals { num_approvals } => {
                StarknetProposalActionWithArgs::SetNumApprovals(Felt::from(num_approvals))
            },
            ProposalAction::SetActiveProposalsLimit { active_proposals_limit } => {
                StarknetProposalActionWithArgs::SetActiveProposalsLimit(Felt::from(active_proposals_limit))
            },
            ProposalAction::SetContextValue { key, value } => {
                StarknetProposalActionWithArgs::SetContextValue(
                    key.chunks(16)
                        .map(|chunk| Felt::from_bytes_be_slice(chunk))
                        .collect(),
                    value.chunks(16)
                        .map(|chunk| Felt::from_bytes_be_slice(chunk))
                        .collect()
                )
            },
        }
    }
}

impl From<StarknetProposal> for Proposal {
    fn from(sp: StarknetProposal) -> Self {
      Proposal {
          id: Repr::new(ProposalId::from_bytes(|bytes| {
              // Take last 16 bytes from high and first 16 bytes from low
              let mut combined = [0u8; 32];
              combined[..16].copy_from_slice(&sp.proposal_id.high.to_bytes_be()[16..]);
              combined[16..].copy_from_slice(&sp.proposal_id.low.to_bytes_be()[..16]);
              bytes.copy_from_slice(&combined);
              Ok(32)
          }).expect("Valid proposal ID")),
          author_id: Repr::new(SignerId::from_bytes(|bytes| {
              // Same for author_id
              let mut combined = [0u8; 32];
              combined[..16].copy_from_slice(&sp.author_id.high.to_bytes_be()[16..]);
              combined[16..].copy_from_slice(&sp.author_id.low.to_bytes_be()[..16]);
              bytes.copy_from_slice(&combined);
              Ok(32)
          }).expect("Valid signer ID")),
          actions: vec![sp.actions.into()],
      }
    }
}

impl From<StarknetProposalActionWithArgs> for ProposalAction {
    fn from(action: StarknetProposalActionWithArgs) -> Self {
        match action {
          StarknetProposalActionWithArgs::ExternalFunctionCall(contract, selector, calldata) => {
              ProposalAction::ExternalFunctionCall {
                  receiver_id: format!("0x{}", hex::encode(contract.to_bytes_be())),
                  method_name: format!("0x{}", hex::encode(selector.to_bytes_be())),
                  args: calldata.iter()
                      .map(|felt| format!("0x{}", hex::encode(felt.to_bytes_be())))
                      .collect::<Vec<_>>()
                      .join(","),
                  deposit: 0,
                  gas: 0,
              }
          },
          StarknetProposalActionWithArgs::Transfer(receiver, amount, _token) => {
              ProposalAction::Transfer {
                  receiver_id: format!("0x{}", hex::encode(receiver.to_bytes_be())),
                  amount: u128::from_be_bytes(amount.low.to_bytes_be()[16..32].try_into().unwrap())
                      + (u128::from_be_bytes(amount.high.to_bytes_be()[16..32].try_into().unwrap()) << 64),
              }
          },
          StarknetProposalActionWithArgs::SetNumApprovals(num) => {
              ProposalAction::SetNumApprovals {
                  num_approvals: u32::from_be_bytes(num.to_bytes_be()[28..32].try_into().unwrap()),
              }
          },
          StarknetProposalActionWithArgs::SetActiveProposalsLimit(limit) => {
              ProposalAction::SetActiveProposalsLimit {
                  active_proposals_limit: u32::from_be_bytes(limit.to_bytes_be()[28..32].try_into().unwrap()),
              }
          },
          StarknetProposalActionWithArgs::SetContextValue(key, value) => {
              ProposalAction::SetContextValue {
                  key: key.iter()
                      .flat_map(|felt| felt.to_bytes_be())
                      .collect(),
                  value: value.iter()
                      .flat_map(|felt| felt.to_bytes_be())
                      .collect(),
              }
          },
          _ => panic!("Unsupported action type"),
        }
    }
}

#[derive(Debug, Decode)]
pub struct StarknetProposalWithApprovals {
    pub proposal_id: StarknetProposalId,
    pub num_approvals: Felt,
}

impl From<StarknetProposalWithApprovals> for ProposalWithApprovals {
    fn from(spa: StarknetProposalWithApprovals) -> Self {
        ProposalWithApprovals {
            proposal_id: Repr::new(ProposalId::from_bytes(|bytes| {
                let mut full_bytes = Vec::with_capacity(64);
                full_bytes.extend_from_slice(&spa.proposal_id.high.to_bytes_be());
                full_bytes.extend_from_slice(&spa.proposal_id.low.to_bytes_be());
                bytes.copy_from_slice(&full_bytes);
                Ok(64)
            }).expect("Valid proposal ID")),
            num_approvals: u32::from_be_bytes(spa.num_approvals.to_bytes_be()[28..32].try_into().unwrap()) as usize,
        }
    }
}

#[derive(Debug, Decode)]
pub struct StarknetApprovers {
    pub approvers: Vec<StarknetIdentity>,
}

impl From<StarknetApprovers> for Vec<ContextIdentity> {
    fn from(sa: StarknetApprovers) -> Self {
        sa.approvers
            .into_iter()
            .map(|identity| {
                ContextIdentity::from_bytes(|bytes| {
                    let mut full_bytes = Vec::with_capacity(64);
                    full_bytes.extend_from_slice(&identity.high.to_bytes_be());
                    full_bytes.extend_from_slice(&identity.low.to_bytes_be());
                    bytes.copy_from_slice(&full_bytes);
                    Ok(64)
                }).expect("Valid identity")
            })
            .collect()
    }
}

