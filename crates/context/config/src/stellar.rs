use soroban_sdk::{
    contracterror, contracttype, symbol_short, Address, Bytes, BytesN, Env, IntoVal, String,
    Symbol, Val, Vec,
};
use stellar_types::FromWithEnv;

use crate::repr::{Repr, ReprBytes, ReprTransmute};
use crate::types::ProposalId;
use crate::{ProposalAction, ProposalWithApprovals, ProxyMutateRequest};

pub mod stellar_repr;
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
                vec_args.push_back(String::from_str(env, &args).into());

                let symbol = symbol_short!("TODO_ALEN");

                StellarProposalAction::ExternalFunctionCall(
                    Address::from_string(&String::from_str(env, &receiver_id)),
                    symbol,
                    vec_args,
                    deposit.try_into().unwrap(),
                )
            }
            ProposalAction::Transfer {
                receiver_id,
                amount,
            } => StellarProposalAction::Transfer(
                Address::from_string(&String::from_str(&env, &receiver_id)),
                amount.try_into().unwrap(),
            ),
            ProposalAction::SetNumApprovals { num_approvals } => {
                StellarProposalAction::SetNumApprovals(num_approvals)
            }
            ProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => StellarProposalAction::SetActiveProposalsLimit(active_proposals_limit),
            ProposalAction::SetContextValue { key, value } => {
                StellarProposalAction::SetContextValue(
                    Bytes::from_slice(&env, &key),
                    Bytes::from_slice(&env, &value),
                )
            }
            ProposalAction::DeleteProposal { proposal_id } => {
                let proposal_id =
                    BytesN::from_array(&env, &proposal_id.rt().expect("infallible conversion"));
                StellarProposalAction::DeleteProposal(proposal_id)
            }
        }
    }
}

impl FromWithEnv<ProxyMutateRequest> for StellarProxyMutateRequest {
    fn from_with_env(request: ProxyMutateRequest, env: &Env) -> Self {
        let request = match request {
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
        };
        request
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
