use soroban_sdk::{
    contracterror, contracttype, Address, Bytes, BytesN, Env, String as SorobanString, Symbol, Val,
    Vec,
};
use stellar_types::FromWithEnv;

use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::ProposalId;
use crate::{Proposal, ProposalAction, ProposalWithApprovals, ProxyMutateRequest};

pub mod stellar_types;

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct StellarIdentity(pub BytesN<32>);

#[derive(Clone, Debug, Eq, PartialEq)]
#[contracttype]
pub struct StellarProposalId(pub BytesN<32>);

#[derive(Clone, Debug)]
#[contracttype]
pub enum StellarProposalAction {
    ExternalFunctionCall(Address, Symbol, Vec<Val>, i128), // receiver_id, method_name, args, deposit
    Transfer(Address, i128),                               // receiver_id, amount
    SetNumApprovals(u32),
    SetActiveProposalsLimit(u32),
    SetContextValue(Bytes, Bytes), // key, value
    DeleteProposal(BytesN<32>),    // proposal_id
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct StellarProposal {
    pub id: BytesN<32>,
    pub author_id: BytesN<32>,
    pub actions: Vec<StellarProposalAction>,
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct StellarProposalWithApprovals {
    pub proposal_id: BytesN<32>,
    pub num_approvals: u32,
}

impl From<StellarProposalWithApprovals> for ProposalWithApprovals {
    fn from(value: StellarProposalWithApprovals) -> Self {
        let proposal_id = ProposalId::from_bytes(|bytes| {
            bytes.copy_from_slice(&value.proposal_id.to_array());
            Ok(32)
        })
        .expect("valid proposal ID");

        ProposalWithApprovals {
            proposal_id: Repr::new(proposal_id),
            num_approvals: value.num_approvals as usize,
        }
    }
}

#[derive(Clone, Debug)]
#[contracttype]
pub struct StellarProposalApprovalWithSigner {
    pub proposal_id: BytesN<32>,
    pub signer_id: BytesN<32>,
}

#[derive(Clone, Debug)]
#[contracttype]
pub enum StellarProxyMutateRequest {
    Propose(StellarProposal),
    Approve(StellarProposalApprovalWithSigner),
}

impl FromWithEnv<ProposalAction> for StellarProposalAction {
    fn from_with_env(action: ProposalAction, env: &Env) -> Self {
        match action {
            ProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
            } => {
                let mut vec_args = Vec::new(env);
                vec_args.push_back(SorobanString::from_str(env, &args).into());

                StellarProposalAction::ExternalFunctionCall(
                    Address::from_string(&SorobanString::from_str(env, &receiver_id)),
                    Symbol::new(env, &method_name),
                    vec_args,
                    deposit as i128,
                )
            }
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => StellarProposalAction::Transfer(
                Address::from_string(&SorobanString::from_str(env, &receiver_id)),
                amount as i128,
            ),
            ProposalAction::SetNumApprovals { num_approvals } => {
                StellarProposalAction::SetNumApprovals(num_approvals)
            }
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => StellarProposalAction::SetActiveProposalsLimit(active_proposals_limit),
            ProposalAction::SetContextValue { key, value } => {
                StellarProposalAction::SetContextValue(
                    Bytes::from_slice(env, &key),
                    Bytes::from_slice(env, &value),
                )
            }
            ProposalAction::DeleteProposal { proposal_id } => {
                StellarProposalAction::DeleteProposal(BytesN::from_array(
                    env,
                    &proposal_id.rt().expect("infallible conversion"),
                ))
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl From<StellarProposalAction> for ProposalAction {
    fn from(action: StellarProposalAction) -> Self {
        match action {
            StellarProposalAction::ExternalFunctionCall(receiver, method, args, deposit) => {
                ProposalAction::ExternalFunctionCall {
                    receiver_id: receiver.to_string().to_string(),
                    method_name: method.to_string(),
                    args: format!("{:?}", args),
                    deposit: deposit as u128,
                }
            }
            StellarProposalAction::Transfer(receiver, amount) => ProposalAction::Transfer {
                receiver_id: receiver.to_string().to_string(),
                amount: amount as u128,
            },
            StellarProposalAction::SetNumApprovals(num) => {
                ProposalAction::SetNumApprovals { num_approvals: num }
            }
            StellarProposalAction::SetActiveProposalsLimit(limit) => {
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit: limit,
                }
            }
            StellarProposalAction::SetContextValue(key, value) => ProposalAction::SetContextValue {
                key: key.to_alloc_vec().into_boxed_slice(),
                value: value.to_alloc_vec().into_boxed_slice(),
            },
            StellarProposalAction::DeleteProposal(id) => ProposalAction::DeleteProposal {
                proposal_id: Repr::new(
                    ProposalId::from_bytes(|dest| {
                        dest.copy_from_slice(&id.to_array());
                        Ok(32)
                    })
                    .expect("infallible conversion"),
                ),
            },
        }
    }
}

impl FromWithEnv<ProxyMutateRequest> for StellarProxyMutateRequest {
    fn from_with_env(request: ProxyMutateRequest, env: &Env) -> Self {
        match request {
            ProxyMutateRequest::Propose { proposal } => {
                let mut actions = Vec::new(&env);
                for action in proposal.actions {
                    let stellar_action = StellarProposalAction::from_with_env(action, env);
                    actions.push_back(stellar_action);
                }
                StellarProxyMutateRequest::Propose(StellarProposal {
                    id: BytesN::from_array(&env, &proposal.id.rt().expect("infallible conversion")),
                    author_id: BytesN::from_array(
                        &env,
                        &proposal.author_id.rt().expect("infallible conversion"),
                    ),
                    actions,
                })
            }
            ProxyMutateRequest::Approve { approval } => {
                StellarProxyMutateRequest::Approve(StellarProposalApprovalWithSigner {
                    proposal_id: BytesN::from_array(
                        &env,
                        &approval.proposal_id.rt().expect("infallible conversion"),
                    ),
                    signer_id: BytesN::from_array(
                        &env,
                        &approval.signer_id.rt().expect("infallible conversion"),
                    ),
                })
            }
        }
    }
}

#[cfg(not(target_family = "wasm"))]
impl From<StellarProposal> for Proposal {
    fn from(proposal: StellarProposal) -> Self {
        Proposal {
            id: proposal.id.rt().expect("infallible conversion"),
            author_id: proposal.author_id.rt().expect("infallible conversion"),
            actions: proposal
                .actions
                .iter()
                .map(|a| ProposalAction::from(a.clone()))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Copy)]
#[contracterror]
pub enum StellarProxyError {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    ProposalNotFound = 3,
    ProposalAlreadyApproved = 4,
    TooManyActiveProposals = 5,
    InvalidAction = 6,
    ExecutionFailed = 7,
    InsufficientBalance = 8,
    TransferFailed = 9,
    CrossContractCallFailed = 10,
    ParseError = 11,
}
