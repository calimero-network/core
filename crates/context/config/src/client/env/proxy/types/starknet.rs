use starknet::core::codec::{Decode, Encode};
use starknet::core::types::{Felt, U256};

use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::{ContextIdentity, ProposalId, SignerId};
use crate::{
    Proposal, ProposalAction, ProposalApprovalWithSigner, ProposalWithApprovals, ProxyMutateRequest,
};

#[derive(Debug, Encode, Decode)]
pub struct FeltPair {
    pub high: Felt,
    pub low: Felt,
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetIdentity(pub FeltPair);

#[derive(Debug, Encode, Decode)]
pub struct StarknetProposalId(pub FeltPair);

#[derive(Debug, Encode, Decode)]
pub struct StarknetU256(pub FeltPair);

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
    pub added_timestamp: Felt,
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
pub enum StarknetProposalActionWithArgs {
    ExternalFunctionCall(Felt, Felt, Vec<Felt>),
    Transfer(Felt, StarknetU256),
    SetNumApprovals(Felt),
    SetActiveProposalsLimit(Felt),
    SetContextValue(Vec<Felt>, Vec<Felt>),
}

#[derive(Debug, Encode, Decode)]
pub struct StarknetSignedRequest {
    pub payload: Vec<Felt>,
    pub signature_r: Felt,
    pub signature_s: Felt,
}

#[derive(Debug, Decode)]
pub struct StarknetProposalWithApprovals {
    pub proposal_id: StarknetProposalId,
    pub num_approvals: Felt,
}

#[derive(Debug, Decode)]
pub struct StarknetApprovers {
    pub approvers: Vec<StarknetIdentity>,
}

#[derive(Debug, Decode)]
pub struct StarknetProposals {
    pub proposals: Vec<StarknetProposal>,
}

impl From<StarknetProposals> for Vec<Proposal> {
    fn from(value: StarknetProposals) -> Self {
        value.proposals.into_iter().map(Into::into).collect()
    }
}

// Conversions for StarknetIdentity
impl From<Repr<SignerId>> for StarknetIdentity {
    fn from(value: Repr<SignerId>) -> Self {
        let bytes = value.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        StarknetIdentity(FeltPair {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        })
    }
}

impl From<StarknetIdentity> for SignerId {
    fn from(value: StarknetIdentity) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

// Conversions for ProposalId
impl From<Repr<ProposalId>> for StarknetProposalId {
    fn from(value: Repr<ProposalId>) -> Self {
        let bytes = value.as_bytes();
        let mid_point = bytes.len().checked_div(2).expect("Length should be even");
        let (high_bytes, low_bytes) = bytes.split_at(mid_point);
        StarknetProposalId(FeltPair {
            high: Felt::from_bytes_be_slice(high_bytes),
            low: Felt::from_bytes_be_slice(low_bytes),
        })
    }
}

impl From<StarknetProposalId> for ProposalId {
    fn from(value: StarknetProposalId) -> Self {
        let FeltPair { high, low } = value.0;
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(&high.to_bytes_be()[16..]);
        bytes[16..].copy_from_slice(&low.to_bytes_be()[16..]);
        bytes.rt().expect("Infallible conversion")
    }
}

// Conversions for U256
impl From<U256> for StarknetU256 {
    fn from(value: U256) -> Self {
        StarknetU256(FeltPair {
            high: Felt::from(value.high()),
            low: Felt::from(value.low()),
        })
    }
}

impl From<u128> for StarknetU256 {
    fn from(value: u128) -> Self {
        StarknetU256(FeltPair {
            high: Felt::ZERO,
            low: Felt::from(value),
        })
    }
}

// Conversions for ProxyMutateRequest
impl From<(Repr<SignerId>, ProxyMutateRequest)> for StarknetProxyMutateRequest {
    fn from((signer_id, request): (Repr<SignerId>, ProxyMutateRequest)) -> Self {
        StarknetProxyMutateRequest {
            signer_id: signer_id.into(),
            kind: request.into(),
        }
    }
}

impl From<ProxyMutateRequest> for StarknetProxyMutateRequestKind {
    fn from(request: ProxyMutateRequest) -> Self {
        match request {
            ProxyMutateRequest::Propose { proposal } => {
                StarknetProxyMutateRequestKind::Propose(proposal.into())
            }
            ProxyMutateRequest::Approve { approval } => {
                StarknetProxyMutateRequestKind::Approve(approval.into())
            }
        }
    }
}

// Conversions for Proposal
impl From<Proposal> for StarknetProposal {
    fn from(proposal: Proposal) -> Self {
        StarknetProposal {
            proposal_id: proposal.id.into(),
            author_id: proposal.author_id.into(),
            actions: proposal.actions.into(),
        }
    }
}

impl From<StarknetProposal> for Proposal {
    fn from(value: StarknetProposal) -> Self {
        Proposal {
            id: Repr::new(value.proposal_id.into()),
            author_id: Repr::new(value.author_id.into()),
            actions: vec![value.actions.into()],
        }
    }
}

// Conversions for ProposalApproval
impl From<ProposalApprovalWithSigner> for StarknetConfirmationRequest {
    fn from(approval: ProposalApprovalWithSigner) -> Self {
        StarknetConfirmationRequest {
            proposal_id: approval.proposal_id.into(),
            signer_id: approval.signer_id.into(),
            added_timestamp: Felt::from(approval.added_timestamp),
        }
    }
}

// Conversions for Actions
impl From<Vec<ProposalAction>> for StarknetProposalActionWithArgs {
    fn from(actions: Vec<ProposalAction>) -> Self {
        let action = actions
            .into_iter()
            .next()
            .expect("At least one action required");
        match action {
            ProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                ..
            } => {
                let args_vec: Vec<String> = serde_json::from_str(&args).unwrap_or_default();
                let felt_args = args_vec
                    .iter()
                    .map(|arg| {
                        if arg.starts_with("0x") {
                            Felt::from_hex_unchecked(arg)
                        } else {
                            Felt::from_bytes_be_slice(arg.as_bytes())
                        }
                    })
                    .collect();

                StarknetProposalActionWithArgs::ExternalFunctionCall(
                    Felt::from_bytes_be_slice(receiver_id.as_bytes()),
                    Felt::from_bytes_be_slice(method_name.as_bytes()),
                    felt_args,
                )
            }
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => StarknetProposalActionWithArgs::Transfer(
                Felt::from_bytes_be_slice(receiver_id.as_bytes()),
                amount.into(),
            ),
            ProposalAction::SetNumApprovals { num_approvals } => {
                StarknetProposalActionWithArgs::SetNumApprovals(Felt::from(num_approvals))
            }
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => StarknetProposalActionWithArgs::SetActiveProposalsLimit(Felt::from(
                active_proposals_limit,
            )),
            ProposalAction::SetContextValue { key, value } => {
                StarknetProposalActionWithArgs::SetContextValue(
                    key.chunks(16).map(Felt::from_bytes_be_slice).collect(),
                    value.chunks(16).map(Felt::from_bytes_be_slice).collect(),
                )
            }
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
                    args: calldata
                        .iter()
                        .map(|felt| format!("0x{}", hex::encode(felt.to_bytes_be())))
                        .collect::<Vec<_>>()
                        .join(","),
                    deposit: 0,
                    gas: 0,
                }
            }
            StarknetProposalActionWithArgs::Transfer(receiver, amount) => {
                let FeltPair { high, low } = amount.0;
                ProposalAction::Transfer {
                    receiver_id: format!("0x{}", hex::encode(receiver.to_bytes_be())),
                    amount: u128::from_be_bytes(low.to_bytes_be()[16..32].try_into().unwrap())
                        + (u128::from_be_bytes(high.to_bytes_be()[16..32].try_into().unwrap())
                            << 64),
                }
            }
            StarknetProposalActionWithArgs::SetNumApprovals(num) => {
                ProposalAction::SetNumApprovals {
                    num_approvals: u32::from_be_bytes(
                        num.to_bytes_be()[28..32].try_into().unwrap(),
                    ),
                }
            }
            StarknetProposalActionWithArgs::SetActiveProposalsLimit(limit) => {
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit: u32::from_be_bytes(
                        limit.to_bytes_be()[28..32].try_into().unwrap(),
                    ),
                }
            }
            StarknetProposalActionWithArgs::SetContextValue(key, value) => {
                ProposalAction::SetContextValue {
                    key: key.iter().flat_map(|felt| felt.to_bytes_be()).collect(),
                    value: value.iter().flat_map(|felt| felt.to_bytes_be()).collect(),
                }
            }
        }
    }
}

impl From<StarknetProposalWithApprovals> for ProposalWithApprovals {
    fn from(value: StarknetProposalWithApprovals) -> Self {
        ProposalWithApprovals {
            proposal_id: Repr::new(value.proposal_id.into()),
            num_approvals: u32::from_be_bytes(
                value.num_approvals.to_bytes_be()[28..32]
                    .try_into()
                    .unwrap(),
            ) as usize,
        }
    }
}

impl From<StarknetApprovers> for Vec<ContextIdentity> {
    fn from(value: StarknetApprovers) -> Self {
        value
            .approvers
            .into_iter()
            .map(|identity| {
                let mut bytes = [0u8; 32];
                bytes[..16].copy_from_slice(&identity.0.high.to_bytes_be()[16..]);
                bytes[16..].copy_from_slice(&identity.0.low.to_bytes_be()[16..]);
                bytes.rt().expect("Infallible conversion")
            })
            .collect()
    }
}
